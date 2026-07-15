//! File manager panel for termide.
//!
//! Provides a smart file manager with git integration, drag selection, and file operations.

mod file_info;
mod file_search;
mod git_status;
mod keyboard;
mod navigation;
mod operations;
mod rendering;
mod selection;
mod tree;
mod utils;
mod vfs_state;

pub use file_info::FileInfo;
use navigation::NavigationState;
use selection::SelectionState;
pub use utils::shared_dir_size_cache;
use vfs_state::VfsState;

/// Build HotkeyTable for the file manager from config.
pub(crate) fn build_fm_hotkey_table(config: &Config) -> HotkeyTable {
    let mut t = HotkeyTable::new();
    let kb = &config.file_manager.keybindings;

    // File operations
    t.insert("rename", &kb.rename);
    t.insert("view", &kb.view);
    t.insert("edit", &kb.edit);
    t.insert("copy", &kb.copy);
    t.insert("move", &kb.move_item);
    t.insert("create_dir", &kb.create_dir);
    t.insert("create_file", &kb.create_file);
    t.insert("delete", &kb.delete);
    t.insert("info", &kb.info);

    // Search
    t.insert("search", &kb.search);
    t.insert("search_content", &kb.search_content);
    t.insert("search_replace", &kb.search_replace);

    // Navigation
    t.insert("refresh", &kb.refresh);
    t.insert("go_parent", &kb.go_parent);
    t.insert("go_home", &kb.go_home);
    t.insert("toggle_selection", &kb.toggle_selection);
    t.insert("select_all", &kb.select_all);
    t.insert("toggle_hidden", &kb.toggle_hidden);

    // open_external: config binding, always ensure O present
    if let Some(ref binding) = kb.open_external {
        let mut keys: Vec<String> = match binding {
            KeyBinding::Single(s) => vec![s.clone()],
            KeyBinding::Multiple(v) => v.clone(),
        };
        if !keys.iter().any(|k| k == "O") {
            keys.push("O".into());
        }
        t.insert("open_external", &Some(KeyBinding::Multiple(keys)));
    } else {
        t.insert(
            "open_external",
            &Some(KeyBinding::Multiple(vec!["O".into(), "Alt+Enter".into()])),
        );
    }
    t.insert("switch_directory", &kb.switch_directory);
    t.insert("go_to_path", &kb.go_to_path);
    t.insert("clipboard_copy", &kb.clipboard_copy);
    t.insert("clipboard_cut", &kb.clipboard_cut);
    t.insert("clipboard_paste", &kb.clipboard_paste);
    t
}

/// Case-insensitive string comparison without allocation.
fn cmp_ignore_case(a: &str, b: &str) -> std::cmp::Ordering {
    a.chars()
        .flat_map(char::to_lowercase)
        .cmp(b.chars().flat_map(char::to_lowercase))
}

/// Sort group key: 0 = directories, 1 = executable files, 2 = regular files.
fn sort_group(entry: &FileEntry) -> u8 {
    if entry.is_dir {
        0
    } else if entry.is_executable {
        1
    } else {
        2
    }
}

/// Sort entries: directories first, then executables, then regular files.
/// Within each group, sort alphabetically (case-insensitive).
fn sort_entries(entries: &mut [FileEntry]) {
    entries.sort_by(|a, b| {
        sort_group(a)
            .cmp(&sort_group(b))
            .then_with(|| cmp_ignore_case(&a.name, &b.name))
    });
}

use anyhow::Result;
use ratatui::{buffer::Buffer, layout::Rect, prelude::Widget, widgets::Paragraph};
use std::any::Any;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use termide_config::{constants, Config, FileManagerSettings, KeyBinding};
use termide_core::{
    CommandResult, HotkeyTable, Panel, PanelCommand, PanelEvent, RenderContext, SessionPanel,
};
use termide_git::{get_git_status_async, GitStatus, GitStatusAsyncResult, GitStatusCache};
use termide_modal::{
    ActionButton, ActiveModal, ConfirmModal, FindBar, FindBarAction, FindBarBtn, FindBarConfig,
    FindField, InfoActionModal, InputModal,
};
use termide_state::{DirSizeResult, PendingAction};
use termide_theme::Theme;
use termide_ui::{clipboard, path_utils, IndexClickTracker, ScrollBar};
use termide_vfs::{VfsEntry, VfsError, VfsFileType, VfsOperation, VfsResult};

/// Which kind of search the inline bar drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchBarKind {
    /// File-name search by glob mask (Ctrl+F): single Find field.
    Name,
    /// In-file content search/replace (Ctrl+Shift+F): Mask/Find/Repl.
    Content,
}

/// Where keyboard focus sits while the inline content bar is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BarFocus {
    /// The bar's input fields / buttons consume keys.
    Input,
    /// The results list below the bar consumes keys (arrows move the cursor).
    Results,
}

/// Smart file manager with advanced features
pub struct FileManager {
    current_path: PathBuf,
    /// Flat tree of all known entries (top-level + expanded subdirectories).
    tree_entries: Vec<tree::TreeEntry>,
    /// Indices into `tree_entries` of currently visible nodes (hides collapsed children).
    visible_indices: Vec<usize>,
    /// Tree-drawing prefixes (├─, └─, │) for each visible node.
    tree_prefixes: Vec<String>,
    /// Set of expanded directory paths (persists across reloads within session).
    expanded_dirs: HashSet<PathBuf>,
    /// Cursor position — index into `visible_indices`.
    selected: usize,
    scroll_offset: usize,
    /// Modal window request (action, modal)
    modal_request: Option<(PendingAction, ActiveModal)>,
    /// Visible area height (updated during rendering)
    visible_height: usize,
    /// Click tracker for double-click detection
    click_tracker: IndexClickTracker,
    /// Selection state (multi-select and drag)
    selection: SelectionState,
    /// Git status cache for the current directory
    git_status_cache: Option<GitStatusCache>,
    /// Channel receiver for async git status loading
    git_status_receiver: Option<mpsc::Receiver<GitStatusAsyncResult>>,
    /// Channel receiver for directory size calculation results (needs to be passed to AppState)
    pub dir_size_receiver: Option<mpsc::Receiver<DirSizeResult>>,
    /// Directories waiting for a bounded size walk, FIFO. Results land
    /// in the process-wide `utils::shared_dir_size_cache()`, not here.
    dir_size_queue: VecDeque<PathBuf>,
    /// Currently running size walk: the path being walked and a ready
    /// channel that signals completion (the result itself goes straight
    /// into the shared cache).
    dir_size_pending: Option<(PathBuf, mpsc::Receiver<()>)>,
    /// Last `shared_dir_size_cache().generation()` value we observed —
    /// when it changes we trigger a redraw so updates from other panels
    /// are picked up without polling individual paths.
    dir_size_cache_generation: u64,
    /// Navigation state (cursor restoration, debouncing)
    navigation: NavigationState,
    /// Git repository root (None = not in git repo)
    /// Used for reference counting when navigating between directories
    git_root: Option<PathBuf>,
    /// Cached theme for rendering
    cached_theme: Theme,
    /// Cached config for rendering
    cached_config: FileManagerSettings,
    /// Cached vim_mode setting for keyboard handling
    vim_mode: bool,
    /// Cached VFS connection timeout in seconds
    cached_vfs_timeout_secs: u64,
    /// VFS state for network filesystem support
    vfs: VfsState,
    /// Whether panel is stale (collapsed, skipping background work)
    is_stale: bool,
    /// Whether to show hidden (dot) files
    show_hidden: bool,
    /// File/content search state (replaces TreeSearchModal results display)
    file_search: Option<file_search::FileSearchState>,
    /// Inline search/replace bar, docked at the top of the panel while open.
    /// `None` when no search is active. Serves both file-name (glob) and
    /// content search — see [`bar_kind`](Self::bar_kind).
    search_bar: Option<FindBar>,
    /// Which kind of search the open bar drives.
    bar_kind: SearchBarKind,
    /// Whether the inline bar's inputs hold focus (vs. the results list below).
    bar_focus: BarFocus,
    /// Screen rect of the results zone below the bar (set during render; used
    /// for mouse hit-testing and PgUp/PgDn page size).
    search_results_area: Option<Rect>,
    /// Hotkey table for configurable keyboard shortcuts
    hotkeys: HotkeyTable,
    /// Pointer of the last Arc<Config> used to build hotkeys (skip rebuild when unchanged)
    last_config_ptr: usize,
    /// Background directory reload result (watcher- or constructor-
    /// triggered, non-blocking).
    async_reload_receiver: Option<mpsc::Receiver<AsyncDirReloadResult>>,
    /// A watcher-driven reload was coalesced away (debounce window or an
    /// in-flight reload) and must be retried so the tail of a change burst
    /// still lands. Set by `start_async_reload` when it skips; drained by
    /// `check_async_reload` once a slot frees up.
    reload_dirty: bool,
    /// Cursor restore state to apply once `async_reload_receiver`
    /// resolves. Set by the navigation-driven path which used to be
    /// synchronous; cleared by `check_async_reload`. `None` means a
    /// watcher-triggered passive refresh — keep cursor where it is.
    pending_dir_load: Option<PendingDirLoad>,
    /// In-flight directory listings for tree-expand. Keyed by the
    /// absolute path of the directory being expanded; while an entry is
    /// present the tree shows a synthetic loading placeholder underneath
    /// that directory. `tick()` polls and replaces the placeholder with
    /// real children when the listing resolves. Covers both remote
    /// (VFS) and local (worker thread on `std::fs::read_dir`) expansions.
    pending_expansions: HashMap<PathBuf, PendingExpand>,
}

/// In-flight directory listing for a tree expansion, regardless of
/// whether the source is a remote VFS operation or a local worker
/// thread.
enum PendingExpand {
    Remote(VfsOperation<Vec<VfsEntry>>),
    Local(mpsc::Receiver<Vec<FileEntry>>),
}

impl PendingExpand {
    /// Drain a result if one is ready. Converts both sources into a
    /// uniform `VfsResult<Vec<FileEntry>>` so `finish_expand` does not
    /// have to care which side produced it. The "." / ".." filter for
    /// VFS is applied here; the panel-state `show_hidden` filter
    /// happens later in `finish_expand`.
    fn try_recv(&self) -> Option<VfsResult<Vec<FileEntry>>> {
        match self {
            Self::Remote(op) => op.try_recv().map(|res| {
                res.map(|entries| {
                    entries
                        .into_iter()
                        .filter(|e| e.name != "." && e.name != "..")
                        .map(FileEntry::from_vfs_entry)
                        .collect()
                })
            }),
            Self::Local(rx) => match rx.try_recv() {
                Ok(entries) => Some(Ok(entries)),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(VfsError::Io(
                    std::io::Error::other("local expand worker disconnected"),
                ))),
            },
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct FileEntry {
    pub name: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub is_executable: bool,
    pub is_readonly: bool,
    pub git_status: GitStatus,
    pub size: Option<u64>,
    pub modified: Option<std::time::SystemTime>,
}

impl FileEntry {
    /// Create FileEntry from VfsEntry (for remote directories).
    pub fn from_vfs_entry(entry: VfsEntry) -> Self {
        Self {
            name: entry.name,
            is_dir: matches!(entry.metadata.file_type, VfsFileType::Directory),
            is_symlink: matches!(entry.metadata.file_type, VfsFileType::Symlink),
            is_executable: entry
                .metadata
                .permissions
                .map(|p| p & 0o111 != 0)
                .unwrap_or(false),
            is_readonly: entry.metadata.readonly,
            git_status: GitStatus::Unmodified, // Remote files don't have git status
            size: if matches!(entry.metadata.file_type, VfsFileType::File) {
                Some(entry.metadata.size)
            } else {
                None
            },
            modified: entry.metadata.modified,
        }
    }
}

/// Result of a background directory reload.
struct AsyncDirReloadResult {
    path: PathBuf,
    entries: Vec<FileEntry>,
}

/// Cursor / selection restoration state to apply once an async directory
/// load completes. Stored on `FileManager` between the moment the load
/// is kicked off and the moment `tick()` sees the worker's result.
#[derive(Default)]
struct PendingDirLoad {
    /// Name to put the cursor on when the load resolves.
    previous_name: Option<String>,
    previous_index: usize,
    previous_scroll_offset: usize,
    /// Names of files that were selected before the load. Empty when
    /// the caller did not request selection preservation.
    selected_names: HashSet<String>,
}

/// Standalone directory reader that can run in a background thread.
/// Takes all needed parameters by value to avoid borrowing issues.
fn read_dir_entries_standalone(
    dir_path: &std::path::Path,
    rel_prefix: &str,
    show_hidden: bool,
    git_status_cache: Option<&GitStatusCache>,
) -> Vec<FileEntry> {
    let mut entries = Vec::new();

    if let Ok(read_dir) = fs::read_dir(dir_path) {
        for entry in read_dir.flatten() {
            if let Ok(metadata) = entry.metadata() {
                let name = entry.file_name().to_string_lossy().into_owned();

                if !show_hidden && name.starts_with('.') {
                    continue;
                }

                let is_symlink = if let Ok(link_metadata) = fs::symlink_metadata(entry.path()) {
                    link_metadata.is_symlink()
                } else {
                    false
                };

                let is_dir = if is_symlink {
                    fs::metadata(entry.path())
                        .map(|m| m.is_dir())
                        .unwrap_or(false)
                } else {
                    metadata.is_dir()
                };

                let git_name = if rel_prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{rel_prefix}/{name}")
                };

                let git_status = if is_dir {
                    git_status_cache
                        .map(|cache| cache.get_directory_status(&git_name))
                        .unwrap_or(GitStatus::Unmodified)
                } else {
                    git_status_cache
                        .map(|cache| cache.get_status(&git_name))
                        .unwrap_or(GitStatus::Unmodified)
                };

                #[cfg(unix)]
                let is_executable = {
                    use std::os::unix::fs::PermissionsExt;
                    metadata.permissions().mode() & 0o111 != 0
                };
                #[cfg(not(unix))]
                let is_executable = false;

                #[cfg(unix)]
                let is_readonly = {
                    use std::os::unix::fs::PermissionsExt;
                    let mode = metadata.permissions().mode();
                    (mode & 0o200) == 0
                };
                #[cfg(not(unix))]
                let is_readonly = metadata.permissions().readonly();

                let size = if metadata.is_file() {
                    Some(metadata.len())
                } else {
                    None
                };
                let modified = metadata.modified().ok();

                entries.push(FileEntry {
                    name,
                    is_dir,
                    is_symlink,
                    is_executable,
                    is_readonly,
                    git_status,
                    size,
                    modified,
                });
            }
        }
    }

    sort_entries(&mut entries);
    entries
}

impl FileManager {
    // ── Tree helpers ───────────────────────────────────────────────────

    /// Number of visible entries (used in place of old `entries.len()`).
    fn visible_count(&self) -> usize {
        self.visible_indices.len()
    }

    /// Get `FileEntry` at a visible index.
    fn entry_at(&self, vis_idx: usize) -> Option<&FileEntry> {
        let tree_idx = *self.visible_indices.get(vis_idx)?;
        Some(&self.tree_entries[tree_idx].file_entry)
    }

    /// Get `TreeEntry` at a visible index.
    fn tree_entry_at(&self, vis_idx: usize) -> Option<&tree::TreeEntry> {
        let tree_idx = *self.visible_indices.get(vis_idx)?;
        Some(&self.tree_entries[tree_idx])
    }

    /// Get full path of entry at a visible index.
    fn path_at(&self, vis_idx: usize) -> Option<&PathBuf> {
        let tree_idx = *self.visible_indices.get(vis_idx)?;
        Some(&self.tree_entries[tree_idx].full_path)
    }

    /// Recompute `visible_indices` and `tree_prefixes` from `tree_entries`.
    fn recompute_visible(&mut self) {
        self.visible_indices = tree::compute_visible(&self.tree_entries);
        self.tree_prefixes = tree::compute_prefixes(&self.tree_entries, &self.visible_indices);
    }

    /// Find visible index by entry name (top-level only for navigation restore).
    fn find_entry_index(&self, name: &str) -> Option<usize> {
        self.visible_indices
            .iter()
            .position(|&ti| self.tree_entries[ti].file_entry.name == name)
    }

    /// Create a new smart file manager
    pub fn new() -> Self {
        let current_path = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        Self::new_with_path(current_path)
    }

    /// Build a FileManager with all fields at their defaults for the given
    /// path and VFS state. The two public constructors differ only in these
    /// two values and their post-init action.
    fn new_common(current_path: PathBuf, vfs: VfsState) -> Self {
        Self {
            current_path,
            tree_entries: Vec::new(),
            visible_indices: Vec::new(),
            tree_prefixes: Vec::new(),
            expanded_dirs: HashSet::new(),
            selected: 0,
            scroll_offset: 0,
            modal_request: None,
            visible_height: 10, // Default value, will be updated during rendering
            click_tracker: IndexClickTracker::new(),
            selection: SelectionState::default(),
            git_status_cache: None,
            git_status_receiver: None,
            dir_size_receiver: None,
            dir_size_queue: VecDeque::new(),
            dir_size_pending: None,
            dir_size_cache_generation: 0,
            navigation: NavigationState::new(),
            git_root: None,
            cached_theme: Theme::default(),
            cached_config: FileManagerSettings::default(),
            vim_mode: false,
            cached_vfs_timeout_secs: 60, // Default, will be updated from config
            vfs,
            is_stale: false,
            show_hidden: true,
            file_search: None,
            search_bar: None,
            bar_kind: SearchBarKind::Content,
            bar_focus: BarFocus::Input,
            search_results_area: None,
            hotkeys: HotkeyTable::default(),
            last_config_ptr: 0,
            async_reload_receiver: None,
            reload_dirty: false,
            pending_dir_load: None,
            pending_expansions: HashMap::new(),
        }
    }

    /// Create a new smart file manager with the specified path
    pub fn new_with_path(current_path: PathBuf) -> Self {
        // Canonicalize to resolve symlinks — ensures paths match notify events
        let current_path = std::fs::canonicalize(&current_path).unwrap_or(current_path);
        let vfs = VfsState::with_path(termide_vfs::VfsPath::local(&current_path), None);
        let mut fm = Self::new_common(current_path, vfs);
        let _ = fm.load_directory();
        fm
    }

    /// Create a new FileManager at a VFS URL (for cloning remote panels)
    pub fn new_with_vfs_url(
        url: &str,
        vfs_manager: std::sync::Arc<termide_vfs::VfsManager>,
    ) -> anyhow::Result<Self> {
        let vfs_path = termide_vfs::parse_vfs_url(url)?;
        let vfs = VfsState::with_path(vfs_path, Some(vfs_manager));

        // current_path is unused for remote panels.
        let mut fm = Self::new_common(PathBuf::from("/"), vfs);

        // Start the directory listing operation for remote paths
        fm.vfs.start_list_dir();

        Ok(fm)
    }

    // ── File/content search methods ─────────────────────────────────────

    /// Start file glob search
    pub fn start_file_search(&mut self, glob_mask: &str, use_regex: bool, case_sensitive: bool) {
        let mut state = file_search::FileSearchState::new_file_glob(self.current_path.clone());
        state.start_file_search(glob_mask, use_regex, case_sensitive);
        self.file_search = Some(state);
    }

    /// Start content search (glob mask + query). `use_regex` treats the query
    /// as a regular expression; otherwise it is matched literally.
    pub fn start_content_search(
        &mut self,
        glob_mask: &str,
        query: &str,
        use_regex: bool,
        case_sensitive: bool,
    ) {
        let max_file_size = self.cached_config.content_search_max_file_size_mb * 1024 * 1024;
        let mut state =
            file_search::FileSearchState::new_content(self.current_path.clone(), max_file_size);
        state.start_content_search(glob_mask, query, use_regex, case_sensitive);
        self.file_search = Some(state);
    }

    /// Navigate to next search result
    pub fn search_next(&mut self) {
        if let Some(ref mut state) = self.file_search {
            state.next_result();
        }
    }

    /// Navigate to previous search result
    pub fn search_prev(&mut self) {
        if let Some(ref mut state) = self.file_search {
            state.prev_result();
        }
    }

    /// Close file search and return to normal tree view
    pub fn close_file_search(&mut self) {
        self.file_search = None;
    }

    /// Set the in-progress replacement text for the content search (drives the
    /// `-old/+new` preview on the cursor match).
    pub fn set_content_replace(&mut self, text: Option<String>) {
        if let Some(ref mut state) = self.file_search {
            state.set_replace_text(text);
        }
    }

    /// Apply `replace_with` to every matched file of the active content search.
    /// Returns (files_changed, occurrences_replaced).
    pub fn replace_all_in_content_results(&mut self, replace_with: &str) -> (usize, usize) {
        match self.file_search.as_ref() {
            Some(state) => state.replace_all(replace_with),
            None => (0, 0),
        }
    }

    /// Close search and apply the selected result.
    /// For FileGlob: navigates to the file in the tree.
    /// For Content: returns `Some(PanelEvent::OpenFileAt { .. })` so the caller can open the file.
    pub fn close_search_with_selection(&mut self) -> Option<PanelEvent> {
        use file_search::SelectedSearchResult;
        let selection = self.file_search.as_ref()?.get_selected_result();
        self.close_file_search();
        match selection? {
            SelectedSearchResult::NavigateToFile(path) => {
                self.navigate_to_file(&path);
                None
            }
            SelectedSearchResult::OpenAtLine { path, line } => Some(PanelEvent::OpenFileAt {
                path,
                line,
                column: 0,
            }),
            SelectedSearchResult::OpenDir(path) => {
                // Enter (cd into) the directory.
                self.current_path = std::fs::canonicalize(&path).unwrap_or(path);
                self.selected = 0;
                self.scroll_offset = 0;
                let _ = self.load_directory();
                None
            }
        }
    }

    // === Inline content search / replace bar (Ctrl+Shift+F) ===

    /// Open the inline content bar. `replace = false` is a pure search
    /// (Ctrl+Shift+F); `replace = true` adds the replacement field
    /// (Ctrl+Shift+H).
    pub fn open_content_bar(&mut self, replace: bool) {
        self.open_search_bar(SearchBarKind::Content, replace);
    }

    /// Open (or re-focus) the inline file-name search bar (single glob field).
    pub fn open_name_bar(&mut self) {
        self.open_search_bar(SearchBarKind::Name, false);
    }

    /// Open the inline bar for `kind`, rebuilding it if the kind or the
    /// search/replace shape changed. Field texts are preserved across a
    /// rebuild. The file manager has no left-hand content, so navigation lives
    /// in the results zone (Tab) rather than Prev/Next buttons.
    fn open_search_bar(&mut self, kind: SearchBarKind, replace: bool) {
        let has_replace = self
            .search_bar
            .as_ref()
            .map(|b| b.has_field(FindField::Replace));
        let rebuild = self.search_bar.is_none()
            || self.bar_kind != kind
            || (kind == SearchBarKind::Content && has_replace != Some(replace));

        if rebuild {
            // Preserve typed text across a rebuild (e.g. Search → Replace).
            let (mask, find, repl) = self
                .search_bar
                .as_ref()
                .map(|b| {
                    (
                        b.mask_text().to_string(),
                        b.find_text().to_string(),
                        b.replace_text().to_string(),
                    )
                })
                .unwrap_or_default();

            let mut bar = match kind {
                SearchBarKind::Name => FindBar::new(FindBarConfig {
                    fields: vec![FindField::Find],
                    // `[Aa]` case + `[.*]` regex, like the content form.
                    buttons: vec![FindBarBtn::Case, FindBarBtn::Regex],
                }),
                SearchBarKind::Content => {
                    let mut fields = vec![FindField::Mask, FindField::Find];
                    // Toggles first (`[Aa] [.*]`), then selection + replace.
                    let mut buttons = vec![FindBarBtn::Case, FindBarBtn::Regex];
                    if replace {
                        fields.push(FindField::Replace);
                        buttons.push(FindBarBtn::SelectAll);
                        buttons.push(FindBarBtn::ReplaceAll);
                    }
                    let mut bar = FindBar::new(FindBarConfig { fields, buttons });
                    // The glob field reads as "Find:" (matching the name
                    // search), the content query as "Text:".
                    bar.set_label(FindField::Mask, "Find: ");
                    bar.set_label(FindField::Find, "Text: ");
                    // The replace-all button acts on selected files → "Replace".
                    if replace {
                        bar.set_button_label(FindBarBtn::ReplaceAll, "Replace");
                    }
                    bar.set_text(
                        FindField::Mask,
                        if mask.is_empty() { "*".into() } else { mask },
                    );
                    bar
                }
            };
            if !find.is_empty() {
                bar.set_text(FindField::Find, find);
            }
            if replace && !repl.is_empty() {
                bar.set_text(FindField::Replace, repl);
            }
            self.search_bar = Some(bar);
            self.bar_kind = kind;
        }
        if let Some(bar) = self.search_bar.as_mut() {
            // Focus the query field (Find = the text/glob you type first).
            bar.focus_field(FindField::Find);
        }
        self.bar_focus = BarFocus::Input;
        self.rerun_search();
        self.sync_bar_status();
    }

    /// Close the inline bar and clear its search results.
    fn close_search_bar(&mut self) -> Vec<PanelEvent> {
        self.search_bar = None;
        self.close_file_search();
        self.bar_focus = BarFocus::Input;
        vec![PanelEvent::NeedsRedraw]
    }

    /// Re-run the search from the bar's current fields (called on every edit /
    /// toggle). An empty query clears the results. Dispatches on the bar kind.
    fn rerun_search(&mut self) {
        let Some(bar) = self.search_bar.as_ref() else {
            return;
        };
        let query = bar.find_text().to_string();
        if query.is_empty() {
            self.file_search = None;
            return;
        }
        match self.bar_kind {
            SearchBarKind::Name => {
                self.start_file_search(&query, bar.use_regex(), bar.case_sensitive())
            }
            SearchBarKind::Content => {
                let mask = bar.mask_text().to_string();
                let mask = if mask.is_empty() { "*" } else { mask.as_str() };
                let use_regex = bar.use_regex();
                let case_sensitive = bar.case_sensitive();
                let replace = bar.replace_text().to_string();
                // Replace bar (it has the Repl field) → per-file selection.
                let replace_mode = bar.has_field(FindField::Replace);
                self.start_content_search(mask, &query, use_regex, case_sensitive);
                self.set_content_replace((!replace.is_empty()).then_some(replace));
                if let Some(s) = self.file_search.as_mut() {
                    s.set_replace_mode(replace_mode);
                }
            }
        }
    }

    /// Update only the replacement-preview text without restarting the search
    /// (editing the Replace field must not re-run the query or reset the
    /// cursor).
    fn sync_replace_preview(&mut self) {
        let replace = match self.search_bar.as_ref() {
            Some(bar) => bar.replace_text().to_string(),
            None => return,
        };
        self.set_content_replace((!replace.is_empty()).then_some(replace));
    }

    /// Build the "replace N matches in M files?" confirmation event from the
    /// **selected** files, or a hint when nothing is selected.
    fn content_replace_all_event(&self) -> Vec<PanelEvent> {
        let Some(bar) = self.search_bar.as_ref() else {
            return vec![];
        };
        let replace = bar.replace_text().to_string();
        let summary = self.file_search.as_ref().map(|s| s.selected_summary());
        match summary {
            Some((files, matches)) if files > 0 && matches > 0 => vec![PanelEvent::ShowConfirm {
                message: termide_i18n::t().replace_confirm_fmt(matches, files),
                on_confirm: termide_core::ConfirmAction::ReplaceInContent(replace),
            }],
            _ => vec![PanelEvent::ShowMessage(
                termide_i18n::t().replace_no_files_selected().to_string(),
            )],
        }
    }

    /// Route a key to the inline bar (called from `handle_key` while the bar is
    /// open). Returns the panel events produced.
    fn handle_search_bar_key(&mut self, key: crossterm::event::KeyEvent) -> Vec<PanelEvent> {
        let events = match self.bar_focus {
            BarFocus::Input => self.handle_bar_input_key(key),
            BarFocus::Results => self.handle_bar_results_key(key),
        };
        self.sync_bar_status();
        events
    }

    /// Refresh the bar's right-aligned status: the "N of M" found counter, and
    /// in replace mode the selected-files / replacement summary.
    fn sync_bar_status(&mut self) {
        let data = self.file_search.as_ref().map(|s| {
            (
                s.get_match_info(),
                s.show_checkboxes,
                s.selected_summary(),
                s.all_selected(),
            )
        });
        if let Some(bar) = self.search_bar.as_mut() {
            let (info, replace_mode, (sel_files, sel_matches), all_selected) =
                data.unwrap_or((None, false, (0, 0), false));
            match info {
                Some((c, t)) => bar.set_match_info(c + 1, t),
                None => bar.clear_match_info(),
            }
            bar.set_select_all(all_selected);
            if replace_mode {
                let total = info.map(|(_, t)| t).unwrap_or(0);
                bar.set_info_text(Some(termide_i18n::t().replace_selection_fmt(
                    sel_files,
                    total,
                    sel_matches,
                )));
            } else {
                bar.set_info_text(None);
            }
        }
    }

    fn handle_bar_input_key(&mut self, key: crossterm::event::KeyEvent) -> Vec<PanelEvent> {
        use crossterm::event::KeyCode;

        // Tab / Shift+Tab switch to the results zone (where arrows walk the
        // matches), like section switching in the git-status panel. Within the
        // bar, fields and buttons are reached with arrow keys.
        if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
            if self.file_search.is_some() {
                self.bar_focus = BarFocus::Results;
            }
            return vec![PanelEvent::NeedsRedraw];
        }

        let Some(mut bar) = self.search_bar.take() else {
            return vec![];
        };
        let action = bar.handle_key(key);
        let field = bar.focused_field();
        self.search_bar = Some(bar);

        match action {
            Some(FindBarAction::QueryChanged) => {
                // Editing the Replace field only refreshes the preview; editing
                // Find/Mask or flipping a toggle re-runs the search.
                if field == Some(FindField::Replace) {
                    self.sync_replace_preview();
                } else {
                    self.rerun_search();
                }
                vec![PanelEvent::NeedsRedraw]
            }
            Some(FindBarAction::Refresh) => {
                // Re-run against current directory contents (Ctrl+R).
                self.rerun_search();
                vec![PanelEvent::NeedsRedraw]
            }
            Some(FindBarAction::Next) => {
                self.search_next();
                vec![PanelEvent::NeedsRedraw]
            }
            Some(FindBarAction::Previous) => {
                self.search_prev();
                vec![PanelEvent::NeedsRedraw]
            }
            Some(FindBarAction::ReplaceAll) => self.content_replace_all_event(),
            Some(FindBarAction::SelectAll) => {
                // Toggle: select all when not everything is selected, else clear.
                if let Some(s) = self.file_search.as_mut() {
                    let all = s.all_selected();
                    s.set_all_selected(!all);
                }
                vec![PanelEvent::NeedsRedraw]
            }
            // Enter on the Replace field replaces all (with confirmation);
            // Enter on Mask/Find jumps focus into the results list.
            Some(FindBarAction::Submit) => {
                if field == Some(FindField::Replace) {
                    self.content_replace_all_event()
                } else {
                    self.bar_focus = BarFocus::Results;
                    vec![PanelEvent::NeedsRedraw]
                }
            }
            Some(FindBarAction::Close) => self.close_search_bar(),
            // No per-match Replace button is configured for the file manager.
            Some(FindBarAction::Replace) | None => vec![PanelEvent::NeedsRedraw],
        }
    }

    fn handle_bar_results_key(&mut self, key: crossterm::event::KeyEvent) -> Vec<PanelEvent> {
        use crossterm::event::KeyCode;
        // Page size = the visible results height (fallback to a sane default).
        let page = self
            .search_results_area
            .map(|r| r.height as usize)
            .unwrap_or(10)
            .max(1);
        match key.code {
            KeyCode::Up => {
                self.search_prev();
                vec![PanelEvent::NeedsRedraw]
            }
            KeyCode::Down => {
                self.search_next();
                vec![PanelEvent::NeedsRedraw]
            }
            KeyCode::PageUp => {
                if let Some(s) = self.file_search.as_mut() {
                    s.page_up(page);
                }
                vec![PanelEvent::NeedsRedraw]
            }
            KeyCode::PageDown => {
                if let Some(s) = self.file_search.as_mut() {
                    s.page_down(page);
                }
                vec![PanelEvent::NeedsRedraw]
            }
            // Replace mode: Space toggles the file's checkbox, `a` toggles all.
            KeyCode::Char(' ') => {
                if let Some(s) = self.file_search.as_mut() {
                    s.toggle_selected_at_cursor();
                }
                vec![PanelEvent::NeedsRedraw]
            }
            KeyCode::Char('a') => {
                if let Some(s) = self.file_search.as_mut() {
                    let all = s.all_selected();
                    s.set_all_selected(!all);
                }
                vec![PanelEvent::NeedsRedraw]
            }
            // Collapse / expand the file group at the cursor (content mode).
            KeyCode::Left | KeyCode::Char('h') => {
                if let Some(s) = self.file_search.as_mut() {
                    s.set_collapse_at_cursor(true);
                }
                vec![PanelEvent::NeedsRedraw]
            }
            KeyCode::Right | KeyCode::Char('l') => {
                if let Some(s) = self.file_search.as_mut() {
                    s.set_collapse_at_cursor(false);
                }
                vec![PanelEvent::NeedsRedraw]
            }
            KeyCode::Tab | KeyCode::BackTab => {
                if let Some(bar) = self.search_bar.as_mut() {
                    bar.focus_first();
                }
                self.bar_focus = BarFocus::Input;
                vec![PanelEvent::NeedsRedraw]
            }
            KeyCode::Enter => {
                // Enter opens the selection (a file header opens the file at its
                // first match); on a directory it toggles collapse instead.
                let opens = self
                    .file_search
                    .as_ref()
                    .and_then(|s| s.get_selected_result())
                    .is_some();
                if opens {
                    let open = self.close_search_with_selection();
                    self.search_bar = None;
                    self.bar_focus = BarFocus::Input;
                    let mut out = vec![PanelEvent::NeedsRedraw];
                    out.extend(open);
                    out
                } else {
                    if let Some(s) = self.file_search.as_mut() {
                        s.toggle_collapse_at_cursor();
                    }
                    vec![PanelEvent::NeedsRedraw]
                }
            }
            KeyCode::Esc => self.close_search_bar(),
            _ => vec![],
        }
    }

    /// Get the VfsManager Arc (for cloning panels)
    pub fn vfs_manager_arc(&self) -> std::sync::Arc<termide_vfs::VfsManager> {
        self.vfs.manager_arc()
    }

    /// Get the current directory
    pub fn get_current_directory(&self) -> PathBuf {
        self.current_path.clone()
    }

    /// Get the git repository root (None if not in a git repo)
    pub fn git_root(&self) -> Option<&PathBuf> {
        self.git_root.as_ref()
    }

    /// Get the currently watched root path (git_root or current_path for non-git)
    pub fn watched_root(&self) -> Option<&PathBuf> {
        self.git_root.as_ref()
    }

    /// Check if absolute path is in a gitignored directory
    /// Uses cached git_status_cache to avoid spawning git processes
    pub fn is_path_ignored(&self, absolute_path: &std::path::Path) -> bool {
        // Need repo root (git_root) and git_status_cache
        let repo_root = match self.git_root.as_ref() {
            Some(root) => root,
            None => return false,
        };
        let cache = match self.git_status_cache.as_ref() {
            Some(cache) => cache,
            None => return false,
        };

        // Convert absolute path to repo-relative
        let relative_path = match absolute_path.strip_prefix(repo_root) {
            Ok(rel) => rel,
            Err(_) => return false,
        };

        // Check if this relative path is ignored
        cache.is_path_in_ignored(relative_path)
    }

    /// Take the watched root (for cleanup when closing)
    pub fn take_watched_root(&mut self) -> Option<PathBuf> {
        self.git_root.take()
    }

    /// Navigate to a specific directory
    pub fn navigate_to(&mut self, path: PathBuf) -> Result<()> {
        // Canonicalize to resolve symlinks — ensures paths match notify events
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        if path.is_dir() {
            self.current_path = path.clone();
            self.vfs.set_path(termide_vfs::VfsPath::local(path));
            self.load_directory()
        } else if let Some(parent) = path.parent() {
            // If path is a file, navigate to its parent directory
            self.current_path = parent.to_path_buf();
            self.vfs.set_path(termide_vfs::VfsPath::local(parent));
            self.load_directory()
        } else {
            Ok(())
        }
    }

    /// Navigate to a VFS URL (supports both local and remote paths).
    ///
    /// Examples:
    /// - `/home/user/documents` - local path
    /// - `sftp://user@host/path` - SFTP remote path
    /// - `ftp://host/path` - FTP remote path
    pub fn navigate_to_url(&mut self, url: &str) -> Result<()> {
        let vfs_path =
            termide_vfs::parse_vfs_url(url).map_err(|e| anyhow::anyhow!("Invalid URL: {}", e))?;

        if vfs_path.is_local() {
            // Local path - use existing navigation
            self.navigate_to(vfs_path.path)
        } else {
            // Remote path - update VFS state and trigger connection/listing
            self.vfs
                .navigate_to(vfs_path.clone())
                .map_err(|e| anyhow::anyhow!("VFS navigation failed: {}", e))?;

            // If already connected, start listing (otherwise connection will trigger it)
            if !self.vfs.is_connecting() && !self.vfs.has_pending_operation() {
                self.vfs.start_list_dir();
            }

            // Don't update current_path yet - wait for listing to complete
            // The path will be synced when tick() succeeds
            Ok(())
        }
    }

    /// Get reference to VFS state (for network filesystem operations).
    pub fn vfs_state(&self) -> &VfsState {
        &self.vfs
    }

    /// Check if current path is a remote (network) filesystem.
    pub fn is_remote(&self) -> bool {
        self.vfs.is_remote()
    }

    /// Get display path (includes protocol for remote paths).
    pub fn display_path(&self) -> String {
        self.vfs.display_path()
    }

    /// Load the contents of the current directory
    pub fn load_directory(&mut self) -> Result<()> {
        // Preserve git_root when navigating within the same repo —
        // clearing it breaks OnGitUpdate/OnFsUpdate handlers.
        // Only clear when leaving the repo (navigate_to() handles re-registration).
        if let Some(ref root) = self.git_root {
            if !self.current_path.starts_with(root) {
                self.git_root = None;
            }
        }

        // Update debounce timestamp to prevent rapid subsequent reloads from being skipped
        self.navigation.last_reload_time = Some(std::time::Instant::now());

        self.load_directory_inner(false)
    }

    /// Force directory reload, bypassing debounce
    pub fn force_reload_directory(&mut self) -> Result<()> {
        // Preserve git_root within the same repo (same as load_directory)
        if let Some(ref root) = self.git_root {
            if !self.current_path.starts_with(root) {
                self.git_root = None;
            }
        }
        // Clear last_reload_time to bypass debounce
        self.navigation.last_reload_time = None;

        // For remote paths, invalidate cache and start async listing
        if self.vfs.is_remote() {
            self.vfs.invalidate_cache();
            self.vfs.start_list_dir();
            // Entries will be populated by tick() when VFS operation completes
            Ok(())
        } else {
            // Explicit reload — drop cached sizes under the current path
            // so the user sees a fresh recomputation.
            utils::shared_dir_size_cache().invalidate_subtree(&self.current_path);
            self.load_directory_inner(false)
        }
    }

    /// Navigate to a specific file - opens its parent directory and selects the file
    pub fn navigate_to_file(&mut self, path: &std::path::Path) {
        if let Some(parent) = path.parent() {
            self.current_path =
                std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
            let _ = self.load_directory();

            // Find and select the file in the list
            if let Some(file_name) = path.file_name() {
                let name_str = file_name.to_string_lossy();
                if let Some(idx) = self.find_entry_index(&name_str) {
                    self.selected = idx;
                    self.adjust_scroll_offset(self.visible_height);
                }
            }
        }
    }

    /// Select an entry by name in the current directory
    pub fn select_by_name(&mut self, name: &std::ffi::OsStr) {
        let name_str = name.to_string_lossy();
        if let Some(idx) = self.find_entry_index(&name_str) {
            self.selected = idx;
            self.adjust_scroll_offset(self.visible_height);
        }
    }

    /// Reload directory preserving selection (with debounce to prevent rapid reloads)
    pub fn reload_directory(&mut self) -> Result<()> {
        const RELOAD_DEBOUNCE_MS: u128 = 300;

        // Debounce: skip if last reload was too recent
        if !self.navigation.should_reload(RELOAD_DEBOUNCE_MS) {
            return Ok(());
        }

        // For remote paths, invalidate cache and start async listing
        // Entries will be populated by tick() when VFS operation completes
        if self.vfs.is_remote() {
            self.vfs.invalidate_cache();
            self.vfs.start_list_dir();
            return Ok(());
        }

        // Explicit reload — drop cached sizes under the current path.
        utils::shared_dir_size_cache().invalidate_subtree(&self.current_path);
        self.load_directory_inner(true)
    }

    /// Start a background directory reload (for watcher-triggered updates).
    /// Reads directory entries in a background thread to avoid blocking the
    /// main tick loop. Call `check_async_reload()` on each tick to apply results.
    fn start_async_reload(&mut self) {
        const RELOAD_DEBOUNCE_MS: u128 = 300;
        if !self.navigation.should_reload(RELOAD_DEBOUNCE_MS) {
            // Too soon after the last reload — remember to retry so a burst's
            // final change isn't lost.
            self.reload_dirty = true;
            return;
        }
        // Don't overlap with an existing async reload
        if self.async_reload_receiver.is_some() {
            self.reload_dirty = true;
            return;
        }
        self.reload_dirty = false;
        let dir_path = self.current_path.clone();
        let show_hidden = self.show_hidden;
        let git_cache = self.git_status_cache.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let entries =
                read_dir_entries_standalone(&dir_path, "", show_hidden, git_cache.as_ref());
            let _ = tx.send(AsyncDirReloadResult {
                path: dir_path,
                entries,
            });
        });
        self.async_reload_receiver = Some(rx);
    }

    /// Check if a background directory reload has completed and apply the result.
    /// Returns `true` if entries were updated.
    pub fn check_async_reload(&mut self) -> bool {
        let rx = match self.async_reload_receiver.take() {
            Some(rx) => rx,
            None => {
                // No reload in flight — retry a burst reload that was coalesced
                // away earlier (start_async_reload re-checks the debounce gate).
                if self.reload_dirty {
                    self.start_async_reload();
                }
                return false;
            }
        };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                // Not ready yet — put receiver back
                self.async_reload_receiver = Some(rx);
                return false;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                // Sender dropped without sending — discard
                return false;
            }
        };

        if result.path != self.current_path {
            self.pending_dir_load = None;
            return false; // Stale result — user navigated away
        }

        // Build entries with ".." prefix
        let mut entries = Vec::new();
        if self.current_path.parent().is_some() {
            entries.push(FileEntry {
                name: "..".to_string(),
                is_dir: true,
                is_symlink: false,
                is_executable: false,
                is_readonly: false,
                git_status: GitStatus::Unmodified,
                size: None,
                modified: None,
            });
        }
        entries.extend(result.entries);

        // If `pending_dir_load` is set, this was a navigation-initiated
        // load — restore the saved cursor/selection. Otherwise we're
        // resolving a passive watcher refresh, so just hold the current
        // cursor by name.
        let pending = self.pending_dir_load.take();
        let (current_name, previous_index, previous_scroll_offset, selected_names) =
            if let Some(p) = pending {
                (
                    p.previous_name,
                    p.previous_index,
                    p.previous_scroll_offset,
                    p.selected_names,
                )
            } else {
                (
                    self.entry_at(self.selected).map(|e| e.name.clone()),
                    self.selected,
                    self.scroll_offset,
                    HashSet::new(),
                )
            };

        self.tree_entries = self.build_top_level_tree(entries);
        self.load_expanded_subtrees();
        self.recompute_visible();

        // If the parallel git-status worker already finished and
        // deposited a cache, its `apply_git_statuses` ran when the
        // tree was still empty. Reapply now that tree_entries is
        // populated so the listing isn't stuck on Unmodified colors.
        if self.git_status_cache.is_some() {
            self.apply_git_statuses();
        }

        if !selected_names.is_empty() {
            for (vis_idx, &tree_idx) in self.visible_indices.iter().enumerate() {
                if selected_names.contains(&self.tree_entries[tree_idx].file_entry.name) {
                    self.selection.select(vis_idx);
                }
            }
        }

        self.restore_cursor(current_name, previous_index, previous_scroll_offset);

        true
    }

    /// Build top-level `tree_entries` from a sorted list of `FileEntry`.
    fn build_top_level_tree(&self, entries: Vec<FileEntry>) -> Vec<tree::TreeEntry> {
        entries
            .into_iter()
            .map(|fe| {
                let full_path = if fe.name == ".." {
                    self.current_path
                        .parent()
                        .unwrap_or(&self.current_path)
                        .to_path_buf()
                } else {
                    self.current_path.join(&fe.name)
                };
                let expanded = if fe.is_dir && fe.name != ".." {
                    let is_expanded = self.expanded_dirs.contains(&full_path);
                    Some(is_expanded)
                } else {
                    None
                };
                tree::TreeEntry {
                    file_entry: fe,
                    full_path,
                    depth: 0,
                    expanded,
                    is_loading: false,
                }
            })
            .collect()
    }

    /// Restore cursor position after entries reload.
    /// Priority: newly created item → navigating down → restore by name → fallback to index.
    fn restore_cursor(
        &mut self,
        current_name: Option<String>,
        previous_index: usize,
        previous_scroll_offset: usize,
    ) {
        let count = self.visible_count();
        // Newly-created cursor restore: prefer matching by full path so
        // an entry nested inside an expanded subdir is found correctly.
        // Fall back to matching by name for older callers that only set
        // the name.
        let created_path = self.navigation.take_newly_created_path();
        let created_name = self.navigation.take_newly_created();
        if created_path.is_some() || created_name.is_some() {
            let mut found: Option<usize> = None;
            if let Some(ref path) = created_path {
                for (vis_idx, &tree_idx) in self.visible_indices.iter().enumerate() {
                    if &self.tree_entries[tree_idx].full_path == path {
                        found = Some(vis_idx);
                        break;
                    }
                }
            }
            if found.is_none() {
                if let Some(ref name) = created_name {
                    found = self.find_entry_index(name);
                }
            }
            if let Some(idx) = found {
                self.selected = idx;
                if self.visible_height > 0 {
                    self.adjust_scroll_offset(self.visible_height);
                }
            } else if count > 0 {
                self.selected = previous_index.min(count - 1);
            }
        } else if self.navigation.check_and_reset_navigating_down() {
            self.selected = 0;
            self.scroll_offset = 0;
        } else if let Some(name) = current_name {
            if let Some(pos) = self.find_entry_index(&name) {
                self.selected = pos;
            } else if count > 0 {
                self.selected = previous_index.min(count - 1);
            }
            if self.visible_height > 0 {
                if count <= self.visible_height {
                    self.scroll_offset = 0;
                } else {
                    let max_scroll = count.saturating_sub(self.visible_height);
                    self.scroll_offset = previous_scroll_offset.min(max_scroll);
                }
                self.adjust_scroll_offset(self.visible_height);
            }
        }
    }

    /// Expand a directory at the given visible index, loading children lazily.
    pub(crate) fn expand_dir(&mut self, vis_idx: usize) {
        let tree_idx = match self.visible_indices.get(vis_idx) {
            Some(&idx) => idx,
            None => return,
        };
        if self.tree_entries[tree_idx].expanded != Some(false) {
            return; // not a collapsed dir
        }

        // Both local and remote expansions go through the same async
        // pipeline now — `begin_expand` picks the right listing source
        // (VFS for remote, a worker thread for local) and inserts the
        // loading placeholder. Real children land via `tick()` once
        // the listing resolves.
        self.begin_expand(tree_idx, vis_idx);
    }

    /// Translate a tree entry's local-style `full_path` into a real
    /// VfsPath rooted on the current remote connection.
    fn remote_vfs_path_for(&self, dir_path: &Path) -> Option<termide_vfs::VfsPath> {
        let base = self.vfs.current_path().clone();
        if let Ok(rel) = dir_path.strip_prefix(&self.current_path) {
            if rel.as_os_str().is_empty() {
                Some(base)
            } else {
                Some(base.join(rel))
            }
        } else {
            // Fallback: treat dir_path itself as the absolute remote path
            // (e.g. for ".." or odd entries). Reuse host/port/user.
            Some(termide_vfs::VfsPath::remote(
                base.protocol,
                base.host.clone().unwrap_or_default(),
                dir_path,
            ))
        }
    }

    /// Start the listing for a directory the user just expanded.
    ///
    /// Inserts a synthetic "Loading…" placeholder under `parent_idx`
    /// and registers the in-flight listing in `pending_expansions`.
    /// The placeholder is replaced with real children once `tick()`
    /// sees the listing resolve, identically for remote (VFS op) and
    /// local (worker thread on `std::fs::read_dir`) panels.
    fn begin_expand(&mut self, parent_idx: usize, vis_idx: usize) {
        let dir_path = self.tree_entries[parent_idx].full_path.clone();
        let depth = self.tree_entries[parent_idx].depth;

        // Mark expanded and remember it for restore-after-reload.
        self.tree_entries[parent_idx].expanded = Some(true);
        self.expanded_dirs.insert(dir_path.clone());

        // Skip if we already have a pending expansion for this directory.
        if self.pending_expansions.contains_key(&dir_path) {
            self.recompute_visible();
            return;
        }

        // If children were already loaded earlier (collapse keeps them
        // in tree_entries to make re-expand instantaneous), don't
        // refetch — otherwise each expand/collapse round-trip would
        // append a duplicate set of children.
        let next_idx = parent_idx + 1;
        let already_loaded = next_idx < self.tree_entries.len()
            && self.tree_entries[next_idx].depth > depth
            && !self.tree_entries[next_idx].is_loading;
        if already_loaded {
            let dir_was_selected = self.selection.items.contains(&vis_idx);
            let saved = self.save_selection_paths();
            self.recompute_visible();
            self.restore_selection_by_paths(&saved);
            if dir_was_selected {
                self.select_descendants(vis_idx);
            }
            return;
        }

        let Some(pending) = self.start_listing(&dir_path) else {
            return;
        };

        let placeholder = tree::TreeEntry {
            file_entry: FileEntry {
                name: "…".to_string(),
                is_dir: false,
                is_symlink: false,
                is_executable: false,
                is_readonly: false,
                git_status: GitStatus::Unmodified,
                size: None,
                modified: None,
            },
            full_path: dir_path.join("__loading__"),
            depth: depth + 1,
            expanded: None,
            is_loading: true,
        };
        let insert_at = parent_idx + 1;
        self.tree_entries.insert(insert_at, placeholder);

        self.pending_expansions.insert(dir_path, pending);

        let dir_was_selected = self.selection.items.contains(&vis_idx);
        let saved = self.save_selection_paths();
        self.recompute_visible();
        self.restore_selection_by_paths(&saved);
        if dir_was_selected {
            self.select_descendants(vis_idx);
        }
    }

    /// Variant of [`Self::begin_expand`] used to restore a previously-
    /// expanded subtree after a reload — there's no `vis_idx` and no
    /// selection to cascade.
    fn kick_off_subtree(&mut self, tree_idx: usize, dir_path: PathBuf, depth: usize) {
        if self.pending_expansions.contains_key(&dir_path) {
            return;
        }
        // Already-loaded guard mirrors `begin_expand`: don't fetch if
        // children are already in the tree.
        let next_idx = tree_idx + 1;
        if next_idx < self.tree_entries.len()
            && self.tree_entries[next_idx].depth > depth
            && !self.tree_entries[next_idx].is_loading
        {
            return;
        }
        let Some(pending) = self.start_listing(&dir_path) else {
            return;
        };
        let placeholder = tree::TreeEntry {
            file_entry: FileEntry {
                name: "…".to_string(),
                is_dir: false,
                is_symlink: false,
                is_executable: false,
                is_readonly: false,
                git_status: GitStatus::Unmodified,
                size: None,
                modified: None,
            },
            full_path: dir_path.join("__loading__"),
            depth: depth + 1,
            expanded: None,
            is_loading: true,
        };
        self.tree_entries.insert(tree_idx + 1, placeholder);
        self.pending_expansions.insert(dir_path, pending);
    }

    /// Pick the right async listing for the current panel mode.
    ///
    /// For remote panels this is the VFS `list_dir`; for local panels
    /// a worker thread runs `read_dir_entries_standalone`. Returning
    /// `None` aborts the expansion — usually because we couldn't
    /// derive a `VfsPath` for a remote tree entry.
    fn start_listing(&self, dir_path: &Path) -> Option<PendingExpand> {
        if self.vfs.is_remote() {
            let vfs_path = self.remote_vfs_path_for(dir_path)?;
            Some(PendingExpand::Remote(
                self.vfs.manager().list_dir(&vfs_path),
            ))
        } else {
            let rel_prefix = dir_path
                .strip_prefix(&self.current_path)
                .ok()
                .and_then(|p| p.to_str())
                .unwrap_or("")
                .to_string();
            let (tx, rx) = mpsc::channel();
            let path_for_worker = dir_path.to_path_buf();
            // Worker uses show_hidden=true so the up-to-date panel
            // state can apply the final filter in `finish_expand`,
            // not whatever snapshot was current at spawn time.
            let git_cache = self.git_status_cache.clone();
            std::thread::spawn(move || {
                let entries = read_dir_entries_standalone(
                    &path_for_worker,
                    &rel_prefix,
                    true,
                    git_cache.as_ref(),
                );
                let _ = tx.send(entries);
            });
            Some(PendingExpand::Local(rx))
        }
    }

    /// Drain any completed pending expansions and substitute placeholders
    /// with real children. Returns true if anything changed (caller will
    /// emit NeedsRedraw).
    fn poll_pending_expansions(&mut self) -> bool {
        if self.pending_expansions.is_empty() {
            return false;
        }
        let keys: Vec<PathBuf> = self.pending_expansions.keys().cloned().collect();
        let mut results: Vec<(PathBuf, VfsResult<Vec<FileEntry>>)> = Vec::new();
        for key in keys {
            if let Some(op) = self.pending_expansions.get(&key) {
                if let Some(res) = op.try_recv() {
                    results.push((key, res));
                }
            }
        }
        let changed = !results.is_empty();
        for (dir_path, result) in results {
            self.pending_expansions.remove(&dir_path);
            self.finish_expand(&dir_path, result);
        }
        if changed {
            let saved = self.save_selection_paths();
            self.recompute_visible();
            self.restore_selection_by_paths(&saved);
        }
        changed
    }

    /// Substitute the loading placeholder under `dir_path` with the real
    /// children returned by the directory listing. On error, leave a
    /// "<error>" placeholder so the user sees the failure rather than
    /// silent nothing. Handles both remote (VFS) and local expansions
    /// uniformly — the producer normalised its result to
    /// `Vec<FileEntry>` already.
    fn finish_expand(&mut self, dir_path: &Path, result: VfsResult<Vec<FileEntry>>) {
        // Locate the parent tree index.
        let Some(parent_idx) = self
            .tree_entries
            .iter()
            .position(|te| te.full_path == dir_path)
        else {
            return;
        };
        let parent_depth = self.tree_entries[parent_idx].depth;

        // Remove the placeholder (single entry right after parent that
        // carries `is_loading == true`).
        let placeholder_idx = parent_idx + 1;
        if placeholder_idx < self.tree_entries.len()
            && self.tree_entries[placeholder_idx].is_loading
            && self.tree_entries[placeholder_idx].depth == parent_depth + 1
        {
            self.tree_entries.remove(placeholder_idx);
        }

        match result {
            Ok(entries) => {
                let mut file_entries: Vec<FileEntry> = entries
                    .into_iter()
                    .filter(|e| self.show_hidden || !e.name.starts_with('.'))
                    .collect();
                file_entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
                    (true, false) => std::cmp::Ordering::Less,
                    (false, true) => std::cmp::Ordering::Greater,
                    _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                });
                let child_depth = parent_depth + 1;
                let dir_path_owned = dir_path.to_path_buf();
                let children: Vec<tree::TreeEntry> = file_entries
                    .into_iter()
                    .map(|fe| {
                        let full_path = dir_path_owned.join(&fe.name);
                        let expanded = if fe.is_dir {
                            let is_exp = self.expanded_dirs.contains(&full_path);
                            Some(is_exp)
                        } else {
                            None
                        };
                        tree::TreeEntry {
                            file_entry: fe,
                            full_path,
                            depth: child_depth,
                            expanded,
                            is_loading: false,
                        }
                    })
                    .collect();
                let n = children.len();
                let insert_at = parent_idx + 1;
                self.tree_entries.splice(insert_at..insert_at, children);
                // Re-trigger expansion for any newly visible directories
                // the user had previously expanded.
                for offset in 0..n {
                    let idx = insert_at + offset;
                    if idx >= self.tree_entries.len() {
                        break;
                    }
                    if self.tree_entries[idx].expanded == Some(true) {
                        let child_path = self.tree_entries[idx].full_path.clone();
                        let child_depth = self.tree_entries[idx].depth;
                        self.kick_off_subtree(idx, child_path, child_depth);
                    }
                }
            }
            Err(e) => {
                let placeholder = tree::TreeEntry {
                    file_entry: FileEntry {
                        name: format!("<error: {e}>"),
                        is_dir: false,
                        is_symlink: false,
                        is_executable: false,
                        is_readonly: false,
                        git_status: GitStatus::Unmodified,
                        size: None,
                        modified: None,
                    },
                    full_path: dir_path.join("__error__"),
                    depth: parent_depth + 1,
                    expanded: None,
                    is_loading: false,
                };
                self.tree_entries.insert(parent_idx + 1, placeholder);
                // Also clear expanded state so user can retry by clicking again.
                self.tree_entries[parent_idx].expanded = Some(false);
                self.expanded_dirs.remove(dir_path);
            }
        }
    }

    /// Collapse a directory at the given visible index.
    pub(crate) fn collapse_dir(&mut self, vis_idx: usize) {
        let tree_idx = match self.visible_indices.get(vis_idx) {
            Some(&idx) => idx,
            None => return,
        };
        if self.tree_entries[tree_idx].expanded != Some(true) {
            return; // not an expanded dir
        }

        // Mark as collapsed (children stay in tree_entries, just hidden by visibility)
        self.tree_entries[tree_idx].expanded = Some(false);
        self.expanded_dirs
            .remove(&self.tree_entries[tree_idx].full_path);

        let saved = self.save_selection_paths();
        self.recompute_visible();
        self.restore_selection_by_paths(&saved);
    }

    /// Toggle expand/collapse for a directory at the given visible index.
    pub(crate) fn toggle_expand(&mut self, vis_idx: usize) {
        let tree_idx = match self.visible_indices.get(vis_idx) {
            Some(&idx) => idx,
            None => return,
        };
        match self.tree_entries[tree_idx].expanded {
            Some(true) => self.collapse_dir(vis_idx),
            Some(false) => self.expand_dir(vis_idx),
            None => {} // not a directory
        }
    }

    /// Jump cursor to the parent directory node in the tree.
    /// Used when pressing Left on a non-directory or on a child of an expanded dir.
    fn jump_to_parent_dir(&mut self) {
        let tree_idx = match self.visible_indices.get(self.selected) {
            Some(&idx) => idx,
            None => return,
        };
        let current_depth = self.tree_entries[tree_idx].depth;
        if current_depth == 0 {
            return;
        }
        // Walk backwards in visible_indices to find the parent (first entry with depth < current)
        for vis_idx in (0..self.selected).rev() {
            let ti = self.visible_indices[vis_idx];
            if self.tree_entries[ti].depth < current_depth {
                self.selected = vis_idx;
                self.adjust_scroll_offset(self.visible_height);
                return;
            }
        }
    }

    /// Save selection as set of paths (survives tree rebuilds).
    fn save_selection_paths(&self) -> HashSet<PathBuf> {
        self.selection
            .items
            .iter()
            .filter_map(|&vis_idx| self.path_at(vis_idx).cloned())
            .collect()
    }

    /// Restore selection from saved paths after tree rebuild.
    fn restore_selection_by_paths(&mut self, saved: &HashSet<PathBuf>) {
        self.selection.items.clear();
        for (vis_idx, &tree_idx) in self.visible_indices.iter().enumerate() {
            if saved.contains(&self.tree_entries[tree_idx].full_path) {
                self.selection.items.insert(vis_idx);
            }
        }
    }

    /// Update entries from VFS directory listing (for remote directories).
    fn update_entries_from_vfs(&mut self, vfs_entries: Vec<VfsEntry>) {
        let previous_index = self.selected;
        let previous_scroll_offset = self.scroll_offset;
        let current_name = self.entry_at(self.selected).map(|e| e.name.clone());

        self.tree_entries.clear();
        self.selected = 0;
        self.scroll_offset = 0;
        self.selection.clear();

        let mut entries = Vec::new();

        // Add ".." entry for parent directory navigation (unless at root)
        if self.vfs.current_path().parent().is_some() {
            entries.push(FileEntry {
                name: "..".to_string(),
                is_dir: true,
                is_symlink: false,
                is_executable: false,
                is_readonly: false,
                git_status: GitStatus::Unmodified,
                size: None,
                modified: None,
            });
        }

        // Convert and add VFS entries
        let mut file_entries: Vec<FileEntry> = vfs_entries
            .into_iter()
            .map(FileEntry::from_vfs_entry)
            .filter(|e| self.show_hidden || !e.name.starts_with('.'))
            .collect();

        sort_entries(&mut file_entries);
        entries.extend(file_entries);

        self.tree_entries = self.build_top_level_tree(entries);
        self.recompute_visible();

        // Clear git status (not applicable for remote files)
        self.git_status_cache = None;
        self.git_root = None;

        self.restore_cursor(current_name, previous_index, previous_scroll_offset);
    }

    /// Internal method to load directory with optional selection preservation.
    ///
    /// Returns immediately. For local paths, the directory read runs on
    /// a worker thread via [`Self::async_reload_receiver`] and the
    /// result is applied by `tick()` through [`Self::check_async_reload`];
    /// the cursor/selection restore info is parked on
    /// [`Self::pending_dir_load`] until then. For remote paths the VFS
    /// list already runs async, so we just kick it off.
    fn load_directory_inner(&mut self, preserve_selection: bool) -> Result<()> {
        // Sync VFS path with current_path for local paths
        if !self.vfs.is_remote() {
            self.vfs
                .set_path(termide_vfs::VfsPath::local(self.current_path.clone()));
        }

        // For remote paths, don't clear entries - keep showing current content while loading
        if self.vfs.is_remote() {
            self.vfs.invalidate_cache();
            self.vfs.start_list_dir();
            return Ok(());
        }

        // Save current file name and index to restore position once the
        // async read completes (see `check_async_reload`).
        let previous_name = self
            .navigation
            .take_previous_dir_name()
            .or_else(|| self.entry_at(self.selected).map(|e| e.name.clone()));
        let previous_index = self.selected;
        let previous_scroll_offset = self.scroll_offset;

        let selected_names: HashSet<String> = if preserve_selection {
            self.selection
                .items
                .iter()
                .filter_map(|&vis_idx| self.entry_at(vis_idx).map(|e| e.name.clone()))
                .collect()
        } else {
            HashSet::new()
        };

        self.tree_entries.clear();
        // Drop the stale `visible_indices` / `tree_prefixes` along with
        // the entries — otherwise the next render would index into an
        // empty `tree_entries` and panic before the worker reports back.
        self.recompute_visible();
        self.selected = 0;
        self.scroll_offset = 0;
        self.selection.clear();
        self.selection.end_drag();
        self.dir_size_queue.clear();

        self.git_status_cache = None;
        self.git_status_receiver = Some(get_git_status_async(self.current_path.clone()));

        self.pending_dir_load = Some(PendingDirLoad {
            previous_name,
            previous_index,
            previous_scroll_offset,
            selected_names,
        });

        // Spawn the read_dir on a worker. `read_dir_entries_standalone`
        // is the same helper the watcher-driven async reload uses; we
        // pass `git_cache = None` here because the git status worker is
        // racing us — the watcher will reapply statuses once the cache
        // is in place.
        let (tx, rx) = mpsc::channel();
        let dir_path = self.current_path.clone();
        let show_hidden = self.show_hidden;
        std::thread::spawn(move || {
            let entries = read_dir_entries_standalone(&dir_path, "", show_hidden, None);
            let _ = tx.send(AsyncDirReloadResult {
                path: dir_path,
                entries,
            });
        });
        self.async_reload_receiver = Some(rx);

        Ok(())
    }

    /// After building top-level tree, kick off async listings for every
    /// directory that was expanded in the previous session. Same
    /// pipeline as a fresh expand — placeholders inserted here are
    /// replaced by real children in `tick()` once each listing
    /// resolves, then `finish_expand` recursively schedules listings
    /// for any newly visible subdirs that were also expanded.
    fn load_expanded_subtrees(&mut self) {
        let dirs: Vec<(usize, PathBuf, usize)> = self
            .tree_entries
            .iter()
            .enumerate()
            .filter_map(|(idx, te)| {
                if te.expanded == Some(true) && te.file_entry.is_dir {
                    Some((idx, te.full_path.clone(), te.depth))
                } else {
                    None
                }
            })
            .collect();
        // Walk in reverse so insertions don't shift indices we still
        // need to process.
        for (tree_idx, dir_path, depth) in dirs.into_iter().rev() {
            self.kick_off_subtree(tree_idx, dir_path, depth);
        }
    }

    /// Get current directory path
    pub fn current_path(&self) -> &std::path::Path {
        &self.current_path
    }

    /// Format file size in human-readable format (public method for external use)
    pub fn format_size_static(bytes: u64) -> String {
        utils::format_size(bytes)
    }
}

impl Panel for FileManager {
    fn name(&self) -> &'static str {
        "file_manager"
    }

    fn title(&self) -> String {
        // Return full path, let smart_truncate_title() handle truncation
        // Use VFS display path for remote paths (includes protocol)
        let path = if self.is_remote() {
            self.display_path()
        } else {
            termide_core::util::shorten_home_path(&self.current_path.display().to_string())
        };

        // Show spinner for VFS loading or git status loading
        if self.vfs.is_loading() {
            let spinner = constants::spinner_frame();
            format!("{} {}", spinner, path)
        } else if self.is_git_status_loading() {
            let spinner = constants::spinner_frame();
            format!("{} {} (git)", spinner, path)
        } else {
            path
        }
    }

    fn prepare_render(&mut self, theme: &termide_theme::Theme, config: &std::sync::Arc<Config>) {
        self.cached_theme = *theme;
        self.vim_mode = config.general.vim_mode;
        self.cached_vfs_timeout_secs = config.vfs.connection_timeout_secs;
        let config_ptr = std::sync::Arc::as_ptr(config) as usize;
        if self.last_config_ptr != config_ptr {
            self.last_config_ptr = config_ptr;
            // `FileManagerSettings` embeds ~32 keybinding Strings; only re-clone
            // when the config Arc actually changes, not every frame.
            self.cached_config = config.file_manager.clone();
            self.hotkeys = build_fm_hotkey_table(config);
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, ctx: &RenderContext) {
        let content_height = area.height as usize;
        self.visible_height = content_height;

        // Inline content bar: dock it at the top, render results below.
        if let Some(mut bar) = self.search_bar.take() {
            let bar_h = bar.height().min(area.height);
            let bar_area = Rect {
                x: area.x,
                y: area.y,
                width: area.width,
                height: bar_h,
            };
            let active = self.bar_focus == BarFocus::Input;
            bar.render(bar_area, buf, &self.cached_theme, active);
            self.search_bar = Some(bar);

            // Pseudographic separator between the form and the results.
            let sep_y = area.y + bar_h;
            let mut drew_sep = false;
            if sep_y < area.y + area.height {
                let style = ratatui::style::Style::default().fg(self.cached_theme.disabled);
                for dx in 0..area.width {
                    buf[(area.x + dx, sep_y)].set_symbol("─").set_style(style);
                }
                drew_sep = true;
            }

            let used = bar_h + u16::from(drew_sep);
            let results_area = Rect {
                x: area.x,
                y: area.y + used,
                width: area.width,
                height: area.height.saturating_sub(used),
            };
            self.search_results_area = Some(results_area);
            if results_area.height > 0 {
                if let Some(ref search) = self.file_search {
                    self.render_search_results(results_area, buf, search, &self.cached_theme);
                }
            }
            return;
        }
        self.search_results_area = None;

        // If file search is active, render search results instead of normal tree
        if let Some(ref search) = self.file_search {
            self.render_search_results(area, buf, search, &self.cached_theme);
            return;
        }

        if self.selected >= self.scroll_offset + content_height {
            self.scroll_offset = self.selected - content_height + 1;
        } else if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        }

        // Calculate available width for file names
        let content_width = area.width as usize;
        let items = self.get_items(
            content_height,
            content_width,
            &self.cached_theme,
            ctx.is_focused,
            &self.cached_config,
        );

        // Render file list content directly (accordion already drew border with title/buttons)
        let paragraph = Paragraph::new(items);

        paragraph.render(area, buf);

        // Render scrollbar on the right border
        if let Some(border_x) = ctx.border_right_x {
            let theme_colors = termide_core::ThemeColors::from(&self.cached_theme);
            ScrollBar::render(
                buf,
                border_x,
                area.y,
                area.height,
                self.scroll_offset,
                content_height,
                self.visible_count(),
                &theme_colors,
                ctx.is_focused,
            );
        }
    }

    fn handle_key(&mut self, chord: termide_core::KeyChord) -> Vec<PanelEvent> {
        let key = chord.raw;
        use keyboard::FmCommand;

        // While the inline search bar is open it owns the keyboard.
        if self.search_bar.is_some() {
            return self.handle_search_bar_key(key);
        }

        // Raw key — HotkeyTable.matches() handles Cyrillic normalization internally.
        let command = FmCommand::from_key_event(key, &self.hotkeys, self.vim_mode);
        self.execute_command(command)
    }

    fn handle_mouse(
        &mut self,
        mouse: crossterm::event::MouseEvent,
        panel_area: Rect,
    ) -> Vec<PanelEvent> {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};

        // A click on the inline content bar is owned by the bar. Areas were
        // recorded in absolute screen coordinates during render, so the click
        // coordinates compare directly.
        if self.search_bar.is_some()
            && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
        {
            let on_bar = self
                .search_bar
                .as_ref()
                .is_some_and(|b| b.click_hits_bar(mouse.column, mouse.row));
            if on_bar {
                let mut bar = self.search_bar.take().unwrap();
                let action = bar.handle_mouse(mouse);
                self.search_bar = Some(bar);
                self.bar_focus = BarFocus::Input;
                return match action {
                    Some(FindBarAction::QueryChanged) => {
                        self.rerun_search();
                        vec![PanelEvent::NeedsRedraw]
                    }
                    Some(FindBarAction::Next) => {
                        self.search_next();
                        vec![PanelEvent::NeedsRedraw]
                    }
                    Some(FindBarAction::Previous) => {
                        self.search_prev();
                        vec![PanelEvent::NeedsRedraw]
                    }
                    Some(FindBarAction::ReplaceAll) => self.content_replace_all_event(),
                    _ => vec![PanelEvent::NeedsRedraw],
                };
            }
        }

        // While the inline search bar is open it owns the panel's mouse: clicks
        // in the results zone move/select the result, double-click opens it (or
        // toggles a file header), and the wheel walks matches.
        if self.search_bar.is_some() {
            if let Some(rarea) = self.search_results_area {
                match mouse.kind {
                    MouseEventKind::ScrollUp => {
                        if let Some(s) = self.file_search.as_mut() {
                            s.prev_result();
                        }
                        return vec![PanelEvent::NeedsRedraw];
                    }
                    MouseEventKind::ScrollDown => {
                        if let Some(s) = self.file_search.as_mut() {
                            s.next_result();
                        }
                        return vec![PanelEvent::NeedsRedraw];
                    }
                    MouseEventKind::Down(MouseButton::Left)
                        if mouse.column >= rarea.x
                            && mouse.column < rarea.x + rarea.width
                            && mouse.row >= rarea.y
                            && mouse.row < rarea.y + rarea.height =>
                    {
                        self.bar_focus = BarFocus::Results;
                        let line = (mouse.row - rarea.y) as usize;
                        let col = mouse.column.saturating_sub(rarea.x) as usize;
                        // A click on the collapse triangle toggles that group;
                        // a click on the selection checkbox toggles selection.
                        if self
                            .file_search
                            .as_mut()
                            .map(|s| {
                                s.toggle_collapse_at_visual_click(line, col)
                                    || s.toggle_selection_at_visual_click(line, col)
                            })
                            .unwrap_or(false)
                        {
                            return vec![PanelEvent::NeedsRedraw];
                        }
                        // Otherwise place the cursor on the clicked row (snaps to
                        // the nearest selectable row).
                        if let Some(s) = self.file_search.as_mut() {
                            s.cursor_at_visual_line(line);
                        }
                        let idx = self.file_search.as_ref().map(|s| s.cursor).unwrap_or(0);

                        if self.click_tracker.is_double_click(&idx) {
                            self.click_tracker.reset();
                            // Double-click opens the selection, or toggles a
                            // directory's collapse when it can't be opened.
                            let opens = self
                                .file_search
                                .as_ref()
                                .and_then(|s| s.get_selected_result())
                                .is_some();
                            if opens {
                                let open = self.close_search_with_selection();
                                self.search_bar = None;
                                self.bar_focus = BarFocus::Input;
                                let mut out = vec![PanelEvent::NeedsRedraw];
                                out.extend(open);
                                return out;
                            } else if let Some(s) = self.file_search.as_mut() {
                                s.toggle_collapse_at_cursor();
                            }
                        } else {
                            self.click_tracker.record(idx);
                        }
                        self.sync_bar_status();
                        return vec![PanelEvent::NeedsRedraw];
                    }
                    _ => {}
                }
                self.sync_bar_status();
            }
            // Don't let other mouse events fall through to the tree handler.
            return vec![];
        }

        // Handle scroll first (works anywhere in panel)
        let visible_height = panel_area.height.saturating_sub(2) as usize;
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.scroll_offset = self.scroll_offset.saturating_sub(3);
                // Keep selected in visible area so render doesn't reset scroll
                if self.selected >= self.scroll_offset + visible_height {
                    self.selected = (self.scroll_offset + visible_height).saturating_sub(1);
                }
                return vec![];
            }
            MouseEventKind::ScrollDown => {
                let max_scroll = self.visible_count().saturating_sub(visible_height);
                self.scroll_offset = (self.scroll_offset + 3).min(max_scroll);
                // Keep selected in visible area so render doesn't reset scroll
                if self.selected < self.scroll_offset {
                    self.selected = self.scroll_offset;
                }
                return vec![];
            }
            MouseEventKind::Up(MouseButton::Left) => {
                // End drag - handle this ALWAYS, even if outside panel
                self.selection.end_drag();
                return vec![];
            }
            _ => {}
        }

        // Check that click is inside content area (not on borders)
        let inner_area = Rect {
            x: panel_area.x + 1,
            y: panel_area.y + 1,
            width: panel_area.width.saturating_sub(2),
            height: panel_area.height.saturating_sub(2),
        };

        // Check that click is inside inner area
        if mouse.column < inner_area.x
            || mouse.column >= inner_area.x + inner_area.width
            || mouse.row < inner_area.y
            || mouse.row >= inner_area.y + inner_area.height
        {
            return vec![];
        }

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // Determine index of clicked item
                let relative_row = (mouse.row - inner_area.y) as usize;
                let clicked_index = self.scroll_offset + relative_row;

                if clicked_index < self.visible_count() {
                    // Check modifiers
                    if mouse.modifiers.contains(KeyModifiers::SHIFT) {
                        // Shift+click - select range from selected to clicked_index
                        let start = self.selected.min(clicked_index);
                        let end = self.selected.max(clicked_index);
                        self.selection.dragged.clear();
                        for i in start..=end {
                            self.selection.select(i);
                            self.selection.dragged.insert(i);
                        }
                        self.selected = clicked_index;
                        self.selection.drag_start = Some(clicked_index);
                        self.selection.start_shift_drag(clicked_index);
                    } else if mouse.modifiers.contains(KeyModifiers::CONTROL) {
                        // Ctrl+click - toggle selection on clicked element
                        self.selection.toggle(clicked_index);
                        self.selected = clicked_index;
                        self.selection.start_ctrl_drag(clicked_index);
                    } else {
                        // Check if click is on the expand/collapse icon area for directories
                        let relative_col = (mouse.column - inner_area.x) as usize;
                        let is_dir_icon_click = if let Some(te) = self.tree_entry_at(clicked_index)
                        {
                            let prefix_width = self
                                .tree_prefixes
                                .get(clicked_index)
                                .map(|p| unicode_width::UnicodeWidthStr::width(p.as_str()))
                                .unwrap_or(0);
                            // Icon is at prefix_width + 1 (attr char) position
                            te.expanded.is_some() && relative_col <= prefix_width + 1
                        } else {
                            false
                        };

                        if is_dir_icon_click {
                            // Click on ▶/▼ icon — toggle expand/collapse
                            self.selected = clicked_index;
                            self.toggle_expand(clicked_index);
                            self.click_tracker.reset();
                        } else {
                            // Check for double click using ClickTracker
                            let is_double_click =
                                self.click_tracker.is_double_click(&clicked_index);

                            if is_double_click {
                                // Double click - open file/directory
                                self.selected = clicked_index;
                                let event = self.enter();
                                self.click_tracker.reset();
                                if let Some(e) = event {
                                    return vec![e];
                                }
                            } else {
                                // Single click - select item
                                self.selected = clicked_index;
                                self.click_tracker.record(clicked_index);
                            }
                        }
                        self.selection.end_drag();
                    }
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                // Handle drag only if there's an active drag mode
                if self.selection.is_dragging() {
                    let relative_row = (mouse.row - inner_area.y) as usize;
                    let current_index = self.scroll_offset + relative_row;

                    if current_index < self.visible_count() {
                        // Process drag will select or toggle based on drag mode
                        self.selection.process_drag(current_index);
                        self.selected = current_index;
                    }
                }
            }
            _ => {}
        }

        vec![]
    }

    fn handle_scroll(&mut self, delta: i32, panel_area: Rect) -> Vec<PanelEvent> {
        let lines = delta.unsigned_abs() as usize * 3; // 3 lines per scroll unit
        let visible_height = panel_area.height.saturating_sub(2) as usize;

        if delta < 0 {
            // Scroll up
            self.scroll_offset = self.scroll_offset.saturating_sub(lines);
            // Keep selected in visible area
            if self.selected >= self.scroll_offset + visible_height {
                self.selected = (self.scroll_offset + visible_height).saturating_sub(1);
            }
        } else {
            // Scroll down
            let max_scroll = self.visible_count().saturating_sub(visible_height);
            self.scroll_offset = (self.scroll_offset + lines).min(max_scroll);
            // Keep selected in visible area
            if self.selected < self.scroll_offset {
                self.selected = self.scroll_offset;
            }
        }
        vec![]
    }

    fn reload(&mut self) -> anyhow::Result<()> {
        // Reload directory contents (preserving selection)
        self.reload_directory()
    }

    fn handle_command(&mut self, cmd: PanelCommand<'_>) -> CommandResult {
        match cmd {
            // Return git repository root (enables registration with watcher)
            PanelCommand::GetRepoRoot => CommandResult::RepoRoot(self.git_root.clone()),
            PanelCommand::GetFsWatchInfo => CommandResult::FsWatchInfo {
                watched_root: self.git_root.clone(),
                current_path: self.current_path.clone(),
                is_git_repo: self.git_root.is_some(),
            },
            PanelCommand::SetFsWatchRoot { root, is_git_repo } => {
                self.git_root = if is_git_repo { root } else { None };
                CommandResult::None
            }
            PanelCommand::OnFsUpdate { changed_path } => {
                let current = self.current_path();

                // For git repos: reload on any change within current directory tree
                // (needed for git status color updates)
                // For non-git dirs: reload only for direct children
                let should_reload = if self.git_root.is_some() {
                    // Git repo: any change within current directory tree updates git status
                    // But skip gitignored paths (like target/) to avoid unnecessary reloads
                    changed_path.starts_with(current) && !self.is_path_ignored(changed_path)
                } else {
                    // Non-git: only direct children or current dir itself
                    changed_path.parent() == Some(current) || changed_path == current
                };

                if should_reload {
                    self.start_async_reload();
                    // The light reload re-reads the listing but reuses the
                    // cached git statuses; recompute them too so badges
                    // reflect the change (a plain working-tree edit emits only
                    // this FS event, never an OnGitUpdate).
                    if self.git_root.is_some() && !self.vfs.is_remote() {
                        self.refresh_git_status();
                    }
                    return CommandResult::NeedsRedraw(true);
                }
                CommandResult::NeedsRedraw(false)
            }
            PanelCommand::Reload | PanelCommand::RefreshDirectory => {
                if self.reload_directory().is_ok() {
                    CommandResult::NeedsRedraw(true)
                } else {
                    CommandResult::NeedsRedraw(false)
                }
            }
            // Handle git status updates from unified watcher
            PanelCommand::OnGitUpdate { repo_paths } => {
                // Check if current directory is within one of the updated repositories
                if let Some(git_root) = &self.git_root {
                    let should_update = repo_paths
                        .iter()
                        .any(|p| git_root.starts_with(p) || p.starts_with(git_root));
                    if should_update {
                        // Reload directory to pick up new/deleted files, and
                        // recompute git status (the light reload only reapplies
                        // the cached statuses).
                        self.start_async_reload();
                        if !self.vfs.is_remote() {
                            self.refresh_git_status();
                        }
                        return CommandResult::NeedsRedraw(true);
                    }
                }
                CommandResult::None
            }
            PanelCommand::MarkStale => {
                // Remote panels don't depend on local fs/git events — never mark stale
                if !self.vfs.is_remote() {
                    self.is_stale = true;
                    return CommandResult::NeedsRedraw(true);
                }
                CommandResult::None
            }
            PanelCommand::RefreshIfStale => {
                if self.is_stale {
                    self.is_stale = false;
                    let _ = self.reload_directory();
                    // Only refresh git status for local panels
                    if !self.vfs.is_remote() {
                        self.refresh_git_status();
                    }
                    CommandResult::NeedsRedraw(true)
                } else {
                    CommandResult::None
                }
            }
            // Commands not applicable to FileManager
            PanelCommand::CheckPendingGitDiff
            | PanelCommand::CheckGitDiffReceiver
            | PanelCommand::CheckExternalModification
            | PanelCommand::Resize { .. }
            | PanelCommand::SetHostFocus { .. }
            | PanelCommand::GetModificationStatus
            | PanelCommand::Save
            | PanelCommand::CloseWithoutSaving
            | PanelCommand::SetGitOperationInProgress { .. }
            | PanelCommand::UpdateRepoPaths { .. }
            | PanelCommand::Paste
            | PanelCommand::PasteText { .. } => CommandResult::None,
        }
    }

    fn needs_close_confirmation(&self) -> Option<String> {
        // FileManager doesn't store critical state by itself
        // Pending batch operations are checked in has_panels_requiring_confirmation()
        None
    }

    fn captures_escape(&self) -> bool {
        // Capture Escape when the inline content bar is open (Esc closes the
        // bar), or there's a pending VFS operation or active selection.
        self.search_bar.is_some()
            || self.vfs.has_pending_operation()
            || !self.selection.items.is_empty()
    }

    fn tick(&mut self) -> Vec<PanelEvent> {
        // --- Always drain async results (even when stale/collapsed) ---
        // VFS and git status receivers must be consumed to prevent stuck spinners.
        // IMPORTANT: never early-return before vfs.tick() — results must always be drained.

        let mut events = Vec::new();

        // Drain any tree-expand list_dir operations that have resolved.
        if self.poll_pending_expansions() {
            events.push(PanelEvent::NeedsRedraw);
        }

        // Check for VFS connection timeout (cancel stuck connections)
        if let Some((status, Some(secs))) = self.vfs.connection_status_with_elapsed() {
            if secs >= self.cached_vfs_timeout_secs {
                log::warn!("VFS connection timeout after {}s", secs);
                if self.vfs.cancel_pending().is_some() {
                    self.current_path = self.vfs.path_buf();
                    let _ = self.load_directory();
                    if !self.is_stale {
                        let t = termide_i18n::t();
                        self.show_info_modal(
                            t.connection_timeout_title(),
                            t.connection_timeout_message(),
                        );
                        events.push(PanelEvent::ClearStatus);
                        events.push(PanelEvent::NeedsRedraw);
                        return events;
                    }
                }
            } else if !self.is_stale {
                // Show connection progress in status bar (no early return — must reach vfs.tick)
                events.push(PanelEvent::ShowMessage(format!("{} {}s", status, secs)));
            }
        }

        // Poll VFS operations for completion
        if let Some(result) = self.vfs.tick() {
            match result {
                Ok(entries) => {
                    self.current_path = self.vfs.path_buf();
                    self.update_entries_from_vfs(entries);
                }
                Err(e) => {
                    log::error!("VFS operation failed: {}", e);
                    // Sync to the path `vfs` restored on failure, but do NOT
                    // reload: the listing operation just failed, and reloading
                    // starts another one. On a dead remote session every retry
                    // fails identically, so an auto-reload here spins an
                    // infinite error→reload→error loop (a new alert per tick).
                    // The previous listing is still shown (remote entries aren't
                    // cleared on failure), so the panel stays consistent; the
                    // user reconnects/refreshes explicitly.
                    self.current_path = self.vfs.path_buf();
                    if !self.is_stale {
                        if self.vfs.is_remote() && e.is_connection_lost() {
                            // Dead remote session — offer Reconnect / open local /
                            // close instead of a dead-end "OK".
                            self.show_connection_error_modal(&format!("{}", e));
                        } else {
                            let t = termide_i18n::t();
                            self.show_info_modal(t.connection_error_title(), &format!("{}", e));
                        }
                    }
                }
            }
            if !self.is_stale {
                events.push(PanelEvent::ClearStatus);
                events.push(PanelEvent::NeedsRedraw);
                return events;
            }
        }

        // A remote symlink resolved to a file — open it in the editor.
        if let Some(remote) = self.vfs.take_resolved_file_open() {
            events.push(PanelEvent::ClearStatus);
            events.push(PanelEvent::OpenRemoteFile(remote.to_url_string()));
            events.push(PanelEvent::NeedsRedraw);
            return events;
        }

        // Drain git status receiver — redraw if statuses changed
        if self.check_git_status_async() && !self.is_stale {
            events.push(PanelEvent::NeedsRedraw);
        }

        // Poll file search results
        let mut search_updated = false;
        if let Some(ref mut search) = self.file_search {
            if search.poll_results() {
                search_updated = true;
                events.push(PanelEvent::NeedsRedraw);
            }
        }
        if search_updated {
            // Refresh the bar counter now that results (and their count) landed.
            self.sync_bar_status();
        }

        // Skip remaining work when collapsed (stale)
        if self.is_stale {
            return vec![];
        }

        // Directory size scheduler. Each panel runs at most one worker,
        // but all panels share the process-wide cache, so overlapping
        // directories are computed once.
        if self.cached_config.dir_size_in_wide_view && self.cached_config.dir_size_budget_ms > 0 {
            let cache = utils::shared_dir_size_cache();

            // 1. Drain the completion signal for our own worker, if any.
            if let Some((_, rx)) = self.dir_size_pending.as_ref() {
                match rx.try_recv() {
                    Ok(()) => {
                        self.dir_size_pending = None;
                        events.push(PanelEvent::NeedsRedraw);
                    }
                    Err(mpsc::TryRecvError::Empty) => {}
                    Err(mpsc::TryRecvError::Disconnected) => {
                        self.dir_size_pending = None;
                    }
                }
            }

            // 2. Pick up results other panels have just produced.
            let gen = cache.generation();
            if gen != self.dir_size_cache_generation {
                self.dir_size_cache_generation = gen;
                events.push(PanelEvent::NeedsRedraw);
            }

            // 3. Start the next walk if we have no worker in flight.
            if self.dir_size_pending.is_none() {
                // Top up the queue lazily: any visible directory that is
                // either missing from the cache or marked stale needs
                // (re)computing. Stale entries keep their old value
                // visible while they wait in the queue.
                if self.dir_size_queue.is_empty() {
                    for te in &self.tree_entries {
                        if te.file_entry.is_dir && te.file_entry.name != ".." {
                            let path = &te.full_path;
                            if cache.get(path).is_none() || cache.is_stale(path) {
                                self.dir_size_queue.push_back(path.clone());
                            }
                        }
                    }
                }

                while let Some(path) = self.dir_size_queue.pop_front() {
                    match cache.claim(&path) {
                        utils::ClaimOutcome::AlreadyCached => {
                            // Sibling panel populated this while we waited;
                            // the generation bump above will trigger a redraw.
                            continue;
                        }
                        utils::ClaimOutcome::InProgress => {
                            // Another panel owns this walk — defer and try
                            // again next tick. Breaking avoids spinning on
                            // a full queue of contended paths.
                            self.dir_size_queue.push_back(path);
                            break;
                        }
                        utils::ClaimOutcome::Claimed => {
                            let budget = std::time::Duration::from_millis(
                                self.cached_config.dir_size_budget_ms,
                            );
                            let (tx, rx) = mpsc::channel();
                            let worker_path = path.clone();
                            std::thread::spawn(move || {
                                let outcome =
                                    utils::calculate_dir_size_bounded(&worker_path, budget);
                                utils::shared_dir_size_cache().complete(worker_path, outcome);
                                let _ = tx.send(());
                            });
                            self.dir_size_pending = Some((path, rx));
                            break;
                        }
                    }
                }
            }
        }

        events
    }

    fn to_session(&self, _session_dir: &std::path::Path) -> Option<SessionPanel> {
        // Save file manager with current directory path or VFS URL
        let path_or_url = self.display_path(); // Returns VFS URL for remote, local path for local

        // Defensive check: ensure remote paths include protocol
        if self.is_remote() && !path_or_url.contains("://") {
            log::warn!(
                "Session save WARNING: Remote path missing protocol. VfsPath details: protocol={:?}, host={:?}, path={:?}",
                self.vfs.current_path().protocol,
                self.vfs.current_path().host,
                self.vfs.current_path().path
            );

            // Try to reconstruct the URL manually
            let vfs_path = self.vfs.current_path();
            let reconstructed = if vfs_path.protocol.is_remote() {
                let mut url = format!("{}://", vfs_path.protocol.scheme());
                if let Some(ref user) = vfs_path.username {
                    url.push_str(user);
                    url.push('@');
                }
                if let Some(ref host) = vfs_path.host {
                    url.push_str(host);
                }
                if let Some(port) = vfs_path.port {
                    url.push(':');
                    url.push_str(&port.to_string());
                }
                url.push_str(&vfs_path.path.display().to_string());
                log::info!("Reconstructed URL: {}", url);
                url
            } else {
                log::error!("VfsPath.protocol is not remote but is_remote() returned true!");
                path_or_url
            };

            Some(SessionPanel::FileManager {
                path_or_url: reconstructed,
            })
        } else {
            Some(SessionPanel::FileManager { path_or_url })
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn get_working_directory(&self) -> Option<PathBuf> {
        Some(self.current_path.clone())
    }

    fn get_working_directory_display(&self) -> Option<String> {
        // For remote paths, return the full URL; for local paths, return the path string
        Some(self.display_path())
    }
}

// Additional methods used by app layer (not part of Panel trait)
impl FileManager {
    /// Take modal window request (if any).
    pub fn take_modal_request(&mut self) -> Option<(PendingAction, ActiveModal)> {
        self.modal_request.take()
    }

    /// Set newly created item name for cursor navigation after reload
    pub fn set_newly_created(&mut self, name: String) {
        self.navigation.set_newly_created(name);
    }

    /// Show an information modal with a message and OK button.
    fn show_info_modal(&mut self, title: &str, message: &str) {
        let t = termide_i18n::t();
        let modal = InfoActionModal::new(
            title,
            vec![("".to_string(), message.to_string())],
            vec![ActionButton::new(t.modal_ok(), "ok")],
        );
        self.modal_request = Some((
            PendingAction::VfsMessage,
            ActiveModal::InfoAction(Box::new(modal)),
        ));
    }

    /// Show the dead-remote-session recovery dialog. The buttons report their
    /// id back through `PendingAction::VfsMessage`, which the app routes to
    /// [`Self::reconnect_remote`] / [`Self::switch_to_local_home`] / panel
    /// close. Dismissing (Esc) leaves the panel on its last listing.
    fn show_connection_error_modal(&mut self, message: &str) {
        let t = termide_i18n::t();
        let modal = InfoActionModal::new(
            t.connection_error_title(),
            vec![("".to_string(), message.to_string())],
            vec![
                ActionButton::new(t.vfs_reconnect(), "reconnect"),
                ActionButton::new(t.vfs_open_local(), "go_local"),
                ActionButton::new(t.vfs_close_panel(), "close"),
            ],
        );
        self.modal_request = Some((
            PendingAction::VfsMessage,
            ActiveModal::InfoAction(Box::new(modal)),
        ));
    }

    /// Reconnect the current remote path with a fresh session (drops the dead
    /// provider first). Driven by the recovery dialog's "Reconnect" button.
    pub fn reconnect_remote(&mut self) {
        self.vfs.reconnect();
    }

    /// Drop the remote connection and show the local home directory. Driven by
    /// the recovery dialog's "Open home (local)" button.
    pub fn switch_to_local_home(&mut self) {
        self.vfs.disconnect(); // evicts the provider and sets the local home path
        self.current_path = self.vfs.path_buf();
        let _ = self.load_directory();
    }

    /// Execute a file manager command and return resulting events.
    fn execute_command(&mut self, command: keyboard::FmCommand) -> Vec<PanelEvent> {
        use keyboard::FmCommand;

        let mut events = Vec::new();

        match command {
            // Navigation
            FmCommand::MoveUp => self.move_up(),
            FmCommand::MoveDown => self.move_down(),
            FmCommand::PageUp => {
                self.selected = self.selected.saturating_sub(self.visible_height);
            }
            FmCommand::PageDown => {
                let max_index = self.visible_count().saturating_sub(1);
                self.selected = (self.selected + self.visible_height).min(max_index);
            }
            FmCommand::GoHome => {
                self.selected = 0;
                self.scroll_offset = 0;
            }
            FmCommand::GoEnd => {
                self.selected = self.visible_count().saturating_sub(1);
            }
            FmCommand::Enter => {
                if let Some(event) = self.enter() {
                    events.push(event);
                }
            }
            FmCommand::GoParent => {
                // Use VfsState for navigation (works for both local and remote paths)
                // navigate_up returns None if already at root - don't refresh in that case
                if let Some(dir_name) = self.vfs.navigate_up() {
                    self.navigation.save_for_going_up(dir_name);
                    // Sync local path with VfsState
                    self.current_path = self.vfs.path_buf();
                    let _ = self.load_directory();
                }
            }
            FmCommand::GoHomeDir => {
                if let Some(home) = dirs::home_dir() {
                    self.current_path = std::fs::canonicalize(&home).unwrap_or(home);
                    let _ = self.load_directory();
                }
            }

            // Selection
            FmCommand::ToggleSelection => {
                self.toggle_selection();
            }
            FmCommand::SelectAll => self.select_all(),
            FmCommand::ClearSelection => {
                // If there's a pending VFS operation, cancel it instead of clearing selection
                if self.vfs.has_pending_operation() {
                    if let Some(message) = self.vfs.cancel_pending() {
                        // Sync FileManager path with VfsState
                        self.current_path = self.vfs.path_buf();
                        let _ = self.load_directory();
                        // Show cancellation modal
                        let t = termide_i18n::t();
                        self.show_info_modal(t.connection_cancelled_title(), &message);
                        events.push(PanelEvent::ClearStatus);
                    }
                } else {
                    self.selection.clear();
                }
            }
            FmCommand::CancelOperation => {
                // Explicitly cancel pending VFS operation
                if let Some(message) = self.vfs.cancel_pending() {
                    // Sync FileManager path with VfsState
                    self.current_path = self.vfs.path_buf();
                    let _ = self.load_directory();
                    // Show cancellation modal
                    let t = termide_i18n::t();
                    self.show_info_modal(t.connection_cancelled_title(), &message);
                    events.push(PanelEvent::ClearStatus);
                }
            }
            FmCommand::MoveUpWithSelection => self.move_up_with_selection(),
            FmCommand::MoveDownWithSelection => self.move_down_with_selection(),
            FmCommand::PageUpWithSelection => self.page_up_with_selection(),
            FmCommand::PageDownWithSelection => self.page_down_with_selection(),
            FmCommand::SelectToHome => self.select_to_home(),
            FmCommand::SelectToEnd => self.select_to_end(),
            FmCommand::MoveUpWithToggle => self.move_up_with_toggle(),
            FmCommand::MoveDownWithToggle => self.move_down_with_toggle(),
            FmCommand::PageUpWithToggle => self.page_up_with_toggle(),
            FmCommand::PageDownWithToggle => self.page_down_with_toggle(),

            // File operations
            FmCommand::NewFile => {
                let t = termide_i18n::t();
                let modal = InputModal::new(t.modal_create_file_title(), "");
                let action = PendingAction::CreateFile {
                    directory: self.current_path.clone(),
                };
                self.modal_request = Some((action, ActiveModal::Input(Box::new(modal))));
            }
            FmCommand::NewDirectory => {
                let t = termide_i18n::t();
                let modal = InputModal::new(t.modal_create_dir_title(), "");
                let action = PendingAction::CreateDirectory {
                    directory: self.current_path.clone(),
                };
                self.modal_request = Some((action, ActiveModal::Input(Box::new(modal))));
            }
            FmCommand::DeleteFiles => {
                if self.is_remote() {
                    // Remote delete - use VfsPath
                    let vfs_paths = self.get_selected_vfs_paths();
                    if !vfs_paths.is_empty() {
                        let t = termide_i18n::t();
                        let title = if vfs_paths.len() == 1 {
                            let file_name = vfs_paths[0]
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| "file".to_string());
                            t.modal_delete_single_title(&file_name)
                        } else {
                            t.modal_delete_multiple_title(vfs_paths.len())
                        };
                        let modal = ConfirmModal::new(&title, "");
                        let action = PendingAction::DeleteRemotePath {
                            paths: vfs_paths,
                            vfs_manager: self.vfs_manager_arc(),
                        };
                        self.modal_request = Some((action, ActiveModal::Confirm(Box::new(modal))));
                    }
                } else {
                    // Local delete - use PathBuf
                    let paths = self.get_selected_paths();
                    if !paths.is_empty() {
                        let t = termide_i18n::t();
                        let title = if paths.len() == 1 {
                            let file_name = path_utils::get_file_name_str(&paths[0]);
                            t.modal_delete_single_title(file_name)
                        } else {
                            t.modal_delete_multiple_title(paths.len())
                        };
                        let modal = ConfirmModal::new(&title, "");
                        let action = PendingAction::DeletePath { paths };
                        self.modal_request = Some((action, ActiveModal::Confirm(Box::new(modal))));
                    }
                }
            }
            FmCommand::CopyFiles => {
                let paths = self.get_selected_paths();
                if !paths.is_empty() {
                    let t = termide_i18n::t();
                    let (message, default_dest) = if paths.len() == 1 {
                        let name = path_utils::get_file_name_str(&paths[0]);
                        // Single file: show full path with filename (user can rename)
                        (
                            t.fm_copy_prompt(name),
                            format!("{}/{}", self.current_path.display(), name),
                        )
                    } else {
                        // Multiple files: directory only (trailing slash)
                        (
                            format!("Copy {} items to:", paths.len()),
                            format!("{}/", self.current_path.display()),
                        )
                    };
                    let modal = InputModal::with_default("Copy", &message, &default_dest);
                    let action = PendingAction::CopyPath {
                        sources: paths,
                        target_directory: None,
                        create_symlink: false,
                        create_relative_symlink: false,
                    };
                    self.modal_request = Some((action, ActiveModal::Input(Box::new(modal))));
                }
            }
            FmCommand::MoveFiles => {
                let paths = self.get_selected_paths();
                if !paths.is_empty() {
                    let t = termide_i18n::t();
                    let (message, default_dest) = if paths.len() == 1 {
                        let name = path_utils::get_file_name_str(&paths[0]);
                        (t.fm_move_prompt(name), name.to_string())
                    } else {
                        (
                            format!("Move {} items to:", paths.len()),
                            format!("{}/", self.current_path.display()),
                        )
                    };
                    let modal = InputModal::with_default("Move", &message, &default_dest);
                    let action = PendingAction::MovePath {
                        sources: paths,
                        target_directory: None,
                    };
                    self.modal_request = Some((action, ActiveModal::Input(Box::new(modal))));
                }
            }
            FmCommand::RenameFile => {
                if let Some(te) = self.tree_entry_at(self.selected) {
                    let entry = &te.file_entry;
                    // Only allow renaming files and directories (not deleted or special entries)
                    if entry.git_status == GitStatus::Deleted {
                        return events;
                    }
                    let filename = entry.name.clone();
                    // For remote panels we must hand the operation layer a
                    // VFS URL pair, not a local PathBuf — otherwise the
                    // move falls through the `is_vfs_url` check and the
                    // file ends up renamed on the *local* filesystem (a
                    // very nasty surprise when local and remote share a
                    // path like /home/$USER).
                    let (source, target_dir) = if self.vfs.is_remote() {
                        let parent = self.vfs.current_path().clone();
                        let src_url = parent.join(&filename).to_url_string();
                        let parent_url = parent.to_url_string();
                        (PathBuf::from(src_url), Some(PathBuf::from(parent_url)))
                    } else {
                        let path = te.full_path.clone();
                        let parent = path.parent().map(|p| p.to_path_buf());
                        (path, parent)
                    };
                    let t = termide_i18n::t();
                    let modal = InputModal::with_default(
                        t.op_type_rename(),
                        t.fm_move_prompt(&filename),
                        &filename,
                    );
                    let action = PendingAction::MovePath {
                        sources: vec![source],
                        target_directory: target_dir,
                    };
                    self.modal_request = Some((action, ActiveModal::Input(Box::new(modal))));
                }
            }
            FmCommand::EditFile => {
                if let Some(event) = self.edit_file() {
                    events.push(event);
                }
            }
            FmCommand::ViewFile => {
                if let Some(event) = self.view_file() {
                    events.push(event);
                }
            }
            FmCommand::OpenExternal => {
                if let Some(event) = self.open_external() {
                    events.push(event);
                }
            }

            // Search
            FmCommand::Search => {
                // File-name search is an inline bar docked in the panel.
                self.open_name_bar();
                events.push(PanelEvent::NeedsRedraw);
            }
            FmCommand::SearchContent => {
                // Content search is an inline bar docked in the panel.
                self.open_content_bar(false);
                events.push(PanelEvent::NeedsRedraw);
            }
            FmCommand::SearchReplace => {
                // Content replace: same inline bar with the Replace field.
                self.open_content_bar(true);
                events.push(PanelEvent::NeedsRedraw);
            }

            // Clipboard
            FmCommand::ClipboardCopy => {
                let paths = self.get_selected_paths();
                if !paths.is_empty() {
                    let text = paths
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join("\n");
                    let _ = clipboard::copy(&text);
                }
            }
            FmCommand::ClipboardCut => {
                let paths = self.get_selected_paths();
                if !paths.is_empty() {
                    let text = paths
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join("\n");
                    let _ = clipboard::cut(&text);
                }
            }
            FmCommand::ClipboardPaste => {
                if let Some(text) = clipboard::paste() {
                    let files: Vec<std::path::PathBuf> = text
                        .lines()
                        .filter(|line| !line.is_empty())
                        .map(std::path::PathBuf::from)
                        .filter(|path| path.exists())
                        .collect();

                    if !files.is_empty() {
                        // Land the paste at the cursor's tree level —
                        // same rule as create_file / create_dir use via
                        // `create_target_dir`. Cursor on a root entry
                        // pastes into `current_path`; cursor inside an
                        // expanded subdir pastes into that subdir.
                        let (local_target, _vfs_target) = self.create_target_dir();
                        let t = termide_i18n::t();
                        let message = t.fm_paste_confirm(
                            files.len(),
                            "Copy",
                            &local_target.display().to_string(),
                        );
                        let action = PendingAction::CopyPath {
                            sources: files,
                            target_directory: Some(local_target),
                            create_symlink: false,
                            create_relative_symlink: false,
                        };
                        let modal =
                            ConfirmModal::new(termide_i18n::t().modal_confirm_title(), &message);
                        self.modal_request = Some((action, ActiveModal::Confirm(Box::new(modal))));
                    }
                }
            }

            // Misc
            FmCommand::ShowFileInfo => self.show_file_info(),
            FmCommand::Refresh => {
                let _ = self.reload_directory();
            }
            FmCommand::ToggleHidden => {
                self.show_hidden = !self.show_hidden;
                let _ = self.reload_directory();
            }
            FmCommand::NextPanel => {
                let modal = ConfirmModal::new("", "");
                self.modal_request = Some((
                    PendingAction::NextPanel,
                    ActiveModal::Confirm(Box::new(modal)),
                ));
            }
            FmCommand::PrevPanel => {
                let modal = ConfirmModal::new("", "");
                self.modal_request = Some((
                    PendingAction::PrevPanel,
                    ActiveModal::Confirm(Box::new(modal)),
                ));
            }
            FmCommand::GoToPath => {
                // Open input modal to enter path or URL (supports sftp://, ftp://, etc.)
                let t = termide_i18n::t();
                // Use directory at cursor position (may differ from panel root in tree view)
                let current_path = if let Some(te) = self.tree_entry_at(self.selected) {
                    if te.file_entry.is_dir {
                        te.full_path.display().to_string()
                    } else {
                        te.full_path
                            .parent()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| self.display_path())
                    }
                } else {
                    self.display_path()
                };
                let modal =
                    InputModal::with_default(t.fm_goto_title(), t.fm_goto_prompt(), &current_path);
                let action = PendingAction::GoToPath {
                    current_directory: self.current_path.clone(),
                };
                self.modal_request = Some((action, ActiveModal::Input(Box::new(modal))));
            }

            FmCommand::SwitchDirectory => {
                return vec![PanelEvent::OpenDirectorySwitcher];
            }

            // Tree expand/collapse
            FmCommand::ExpandDir => {
                if let Some(te) = self.tree_entry_at(self.selected) {
                    if te.expanded == Some(false) {
                        self.expand_dir(self.selected);
                    }
                }
            }
            FmCommand::CollapseDir => {
                // If current item is an expanded dir, collapse it
                // If current item is inside an expanded subtree, jump to parent dir
                if let Some(te) = self.tree_entry_at(self.selected) {
                    if te.expanded == Some(true) {
                        self.collapse_dir(self.selected);
                    } else if te.depth > 0 {
                        // Navigate up to parent directory in tree
                        self.jump_to_parent_dir();
                    }
                }
            }

            // No operation
            FmCommand::None => {}
        }

        events
    }
}

impl Default for FileManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use termide_core::{CommandResult, Panel, PanelCommand};

    fn create_file_manager_in_temp() -> (FileManager, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let fm = FileManager::new_with_path(temp_dir.path().to_path_buf());
        (fm, temp_dir)
    }

    #[test]
    fn test_file_manager_new() {
        let (fm, temp_dir) = create_file_manager_in_temp();
        assert_eq!(fm.current_path(), temp_dir.path());
    }

    #[test]
    fn content_bar_opens_focused_on_input_and_esc_closes() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let (mut fm, _t) = create_file_manager_in_temp();

        fm.open_content_bar(false);
        assert!(fm.search_bar.is_some());
        assert_eq!(fm.bar_focus, BarFocus::Input);

        fm.handle_search_bar_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(fm.search_bar.is_none());
        assert!(fm.file_search.is_none());
    }

    #[test]
    fn typing_query_starts_search_and_enter_focuses_results() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let (mut fm, _t) = create_file_manager_in_temp();

        fm.open_content_bar(false); // focus lands on the Find field
        fm.handle_search_bar_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert!(
            fm.file_search.is_some(),
            "typing a query should start a content search"
        );

        // Enter on Find jumps focus into the results list for arrow navigation.
        fm.handle_search_bar_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(fm.bar_focus, BarFocus::Results);
    }

    #[test]
    fn clearing_query_clears_results() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let (mut fm, _t) = create_file_manager_in_temp();

        fm.open_content_bar(false);
        fm.handle_search_bar_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert!(fm.file_search.is_some());

        fm.handle_search_bar_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert!(
            fm.file_search.is_none(),
            "emptying the query should clear the results"
        );
    }

    #[test]
    fn test_handle_command_get_fs_watch_info() {
        let (mut fm, temp_dir) = create_file_manager_in_temp();

        let result = fm.handle_command(PanelCommand::GetFsWatchInfo);
        if let CommandResult::FsWatchInfo {
            current_path,
            is_git_repo,
            ..
        } = result
        {
            assert_eq!(current_path, temp_dir.path());
            assert!(!is_git_repo);
        } else {
            panic!("Expected FsWatchInfo result");
        }
    }

    #[test]
    fn test_handle_command_set_fs_watch_root() {
        let (mut fm, _temp_dir) = create_file_manager_in_temp();

        let root = PathBuf::from("/some/root");
        let result = fm.handle_command(PanelCommand::SetFsWatchRoot {
            root: Some(root.clone()),
            is_git_repo: true,
        });
        assert!(matches!(result, CommandResult::None));

        // Verify the root was set
        let info = fm.handle_command(PanelCommand::GetFsWatchInfo);
        if let CommandResult::FsWatchInfo {
            watched_root,
            is_git_repo,
            ..
        } = info
        {
            assert_eq!(watched_root, Some(root));
            assert!(is_git_repo);
        }
    }

    #[test]
    fn test_handle_command_refresh_directory() {
        let (mut fm, _temp_dir) = create_file_manager_in_temp();

        let result = fm.handle_command(PanelCommand::RefreshDirectory);
        assert!(result.needs_redraw());
    }

    #[test]
    fn test_handle_command_reload() {
        let (mut fm, _temp_dir) = create_file_manager_in_temp();

        let result = fm.handle_command(PanelCommand::Reload);
        assert!(result.needs_redraw());
    }

    #[test]
    fn test_handle_command_get_repo_root() {
        let (mut fm, _temp_dir) = create_file_manager_in_temp();

        // GetRepoRoot returns None when not in git repo
        let result = fm.handle_command(PanelCommand::GetRepoRoot);
        assert!(matches!(result, CommandResult::RepoRoot(None)));

        // Set git_root and verify it's returned
        fm.git_root = Some(PathBuf::from("/test/repo"));
        let result = fm.handle_command(PanelCommand::GetRepoRoot);
        if let CommandResult::RepoRoot(Some(root)) = result {
            assert_eq!(root, PathBuf::from("/test/repo"));
        } else {
            panic!("Expected RepoRoot result");
        }
    }

    #[test]
    fn test_handle_command_not_applicable() {
        let (mut fm, _temp_dir) = create_file_manager_in_temp();

        // Commands not applicable to FileManager should return None
        let result = fm.handle_command(PanelCommand::GetModificationStatus);
        assert!(matches!(result, CommandResult::None));

        let result = fm.handle_command(PanelCommand::Save);
        assert!(matches!(result, CommandResult::None));

        let result = fm.handle_command(PanelCommand::Resize { rows: 24, cols: 80 });
        assert!(matches!(result, CommandResult::None));
    }

    #[test]
    fn test_file_manager_panel_trait_title() {
        let (fm, temp_dir) = create_file_manager_in_temp();
        let title = fm.title();
        // Title may shorten home prefix to ~, so compare against both forms
        let full_path = temp_dir.path().display().to_string();
        let shortened = termide_core::util::shorten_home_path(&full_path);
        assert!(
            title.contains(&full_path) || title.contains(&shortened),
            "title {:?} should contain {:?} or {:?}",
            title,
            full_path,
            shortened,
        );
    }

    #[test]
    fn test_file_manager_panel_trait_needs_close_confirmation() {
        let (fm, _temp_dir) = create_file_manager_in_temp();
        // FileManager doesn't need close confirmation by default
        assert!(fm.needs_close_confirmation().is_none());
    }
}
