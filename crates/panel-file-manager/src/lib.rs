//! File manager panel for termide.
//!
//! Provides a smart file manager with git integration, drag selection, and file operations.

mod command_dispatch;
mod dir_load;
mod expansion;
mod file_info;
mod file_search;
mod git_status;
mod keyboard;
mod navigation;
mod operations;
mod rendering;
mod search_bar;
mod selection;
mod tree;
mod utils;
mod vfs_state;

use command_dispatch::build_fm_hotkey_table;
use dir_load::{AsyncDirReloadResult, PendingDirLoad};
use expansion::PendingExpand;
pub use file_info::FileInfo;
use navigation::NavigationState;
use search_bar::{BarFocus, SearchBarKind};
use selection::SelectionState;
pub use utils::shared_dir_size_cache;
use vfs_state::VfsState;

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
use std::path::PathBuf;
use std::sync::mpsc;

use termide_config::{constants, Config, FileManagerSettings};
use termide_core::{
    CommandResult, HotkeyTable, Panel, PanelCommand, PanelEvent, RenderContext, SessionPanel,
};
use termide_git::{GitStatus, GitStatusAsyncResult, GitStatusCache};
use termide_modal::{ActionButton, ActiveModal, FindBar, FindBarAction, InfoActionModal};
use termide_state::{DirSizeResult, PendingAction};
use termide_theme::Theme;
use termide_ui::{IndexClickTracker, ScrollBar};
use termide_vfs::{VfsEntry, VfsFileType};

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
    /// Memo of the last directory whose immediate child count was read for the
    /// status bar: `(path, count)`. Avoids a `read_dir` on every redraw while
    /// the cursor stays on the same folder; refreshed when it moves.
    dir_item_count_memo: Option<(PathBuf, usize)>,
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
            dir_item_count_memo: None,
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
                    let should_update = termide_git::repo_paths_overlap(git_root, repo_paths);
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
            // Global clipboard routed to the focused panel.
            PanelCommand::Copy => {
                self.clipboard_copy_selection();
                CommandResult::Handled(true)
            }
            PanelCommand::Cut => {
                self.clipboard_cut_selection();
                CommandResult::Handled(true)
            }
            PanelCommand::Paste => {
                self.clipboard_paste_files();
                CommandResult::Handled(true)
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
