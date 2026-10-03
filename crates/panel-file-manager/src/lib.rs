//! File manager panel for termide.
//!
//! Provides a smart file manager with git integration, drag selection, and file operations.

mod background;
mod command_dispatch;
mod dir_load;
mod expansion;
mod file_info;
mod file_search;
mod git_status;
mod keyboard;
mod mouse;
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
pub use operations::{is_database_file, is_raster_image, CreateOutcome};
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
    CommandResult, HotkeyTable, Panel, PanelCommand, PanelEvent, PanelState, RenderContext,
};
use termide_git::{GitStatus, GitStatusAsyncResult, GitStatusCache};
use termide_modal::{ActionButton, ActiveModal, FindBar, InfoActionModal};
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
    /// Set of expanded directory paths (persists across reloads within a run).
    expanded_dirs: HashSet<PathBuf>,
    /// Cursor position — index into `visible_indices`.
    selected: usize,
    scroll_offset: usize,
    /// Scrollbars drawn by the last render, for mouse thumb dragging.
    scrollbars: termide_core::ScrollBars,
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

    /// The entry under the cursor, unless it is the placeholder shown while a
    /// directory is still being listed: that row stands for no file, so no
    /// file action may take it for one.
    fn entry_under_cursor(&self) -> Option<&tree::TreeEntry> {
        self.tree_entry_at(self.selected)
            .filter(|te| !te.is_loading)
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
            scrollbars: termide_core::ScrollBars::default(),
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
        let current_path = dunce::canonicalize(&current_path).unwrap_or(current_path);
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
        let path = dunce::canonicalize(&path).unwrap_or(path);
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
                .navigate_to(vfs_path)
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

    /// At the root of a local Windows drive or share, where going up means
    /// picking another drive rather than a parent directory.
    pub(crate) fn at_local_drive_root(&self) -> bool {
        !self.is_remote() && termide_vfs::is_drive_root(&self.current_path)
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
                dunce::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
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
        // Cleared up front so the branches below that never reach the
        // scrollbar (search results, inline bar) leave no stale geometry for
        // the mouse dispatcher to hit-test against.
        self.scrollbars = termide_core::ScrollBars::default();

        // Inline content bar: dock it at the bottom, render results above.
        if let Some(mut bar) = self.search_bar.take() {
            let bar_h = bar.height().min(area.height);
            let bar_area = Rect {
                x: area.x,
                y: area.y + area.height - bar_h,
                width: area.width,
                height: bar_h,
            };
            let active = self.bar_focus == BarFocus::Input;
            bar.render(bar_area, buf, &self.cached_theme, active);
            self.search_bar = Some(bar);

            // The bar draws its own titled top border, which is the divider.
            let results_area = Rect {
                x: area.x,
                y: area.y,
                width: area.width,
                height: area.height.saturating_sub(bar_h),
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
            self.scrollbars.vertical = ScrollBar::render_tracked(
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
            return self.handle_search_bar_key(chord);
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
        self.on_mouse(mouse, panel_area)
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
            // `Cmd+V` on macOS: the terminal emulator answers the key and
            // types the clipboard's text flavor in. Take it only when that
            // text names real files, so a prose paste is not swallowed. With
            // the search bar open the panel still declines — the bar reads
            // typed keys and drops a bracketed paste either way, and spending
            // the keystroke on a copy from under it would be worse.
            PanelCommand::PasteText { text } => {
                let paste_as_files = self.search_bar.is_none() && self.clipboard_paste_text(&text);
                CommandResult::Handled(paste_as_files)
            }
            PanelCommand::GetScrollBars => CommandResult::ScrollBars(self.scrollbars),
            PanelCommand::SetScrollOffset { offset, .. } => {
                self.scroll_offset = offset;
                // Keep the cursor inside the viewport, the same way wheel
                // scrolling does.
                let visible_height = self.visible_height.max(1);
                if self.selected < self.scroll_offset {
                    self.selected = self.scroll_offset;
                } else if self.selected >= self.scroll_offset + visible_height {
                    self.selected = (self.scroll_offset + visible_height).saturating_sub(1);
                }
                CommandResult::NeedsRedraw(true)
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
            | PanelCommand::ShowGitLog { .. }
            | PanelCommand::SelectionMade { .. }
            | PanelCommand::ChecklistDone { .. }
            | PanelCommand::InputSubmitted { .. }
            | PanelCommand::Confirmed { .. } => CommandResult::None,
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
        self.on_tick()
    }

    fn to_state(&self, _project_dir: &std::path::Path) -> Option<PanelState> {
        // Save file manager with current directory path or VFS URL
        let path_or_url = self.display_path(); // Returns VFS URL for remote, local path for local

        // Defensive check: ensure remote paths include protocol
        if self.is_remote() && !path_or_url.contains("://") {
            log::warn!(
                "Layout save WARNING: Remote path missing protocol. VfsPath details: protocol={:?}, host={:?}, path={:?}",
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

            Some(PanelState::FileManager {
                path_or_url: reconstructed,
            })
        } else {
            Some(PanelState::FileManager { path_or_url })
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

    /// Ask for the password of the encrypted archive whose root is `archive`;
    /// `wrong` says the last one was rejected.
    fn request_archive_password(&mut self, archive: termide_vfs::VfsPath, wrong: bool) {
        let t = termide_i18n::t();
        let name = archive
            .container()
            .and_then(|c| c.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let prompt = if wrong {
            t.fm_archive_password_wrong(&name)
        } else {
            t.fm_archive_password_prompt(&name)
        };
        let modal =
            termide_modal::InputModal::new(t.modal_archive_password_title(), &prompt).password();
        self.modal_request = Some((
            PendingAction::ArchivePassword { archive },
            ActiveModal::Input(Box::new(modal)),
        ));
    }

    /// Open the encrypted archive whose root is `archive` with `password`,
    /// the answer to [`Self::request_archive_password`].
    pub fn open_archive_with_password(&mut self, archive: termide_vfs::VfsPath, password: String) {
        self.navigation.prepare_for_going_down();
        self.vfs.enter_archive_with_password(archive, password);
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

    /// The temp directory as the panel reports it.
    ///
    /// `FileManager` canonicalizes the path it is given, and on macOS the
    /// temp directory lives under `/var`, a symlink to `/private/var`. Compare
    /// against the raw `TempDir::path()` and the assertion fails there while
    /// passing on Linux, where `/tmp` is a real directory.
    fn canonical_temp_path(temp_dir: &TempDir) -> std::path::PathBuf {
        dunce::canonicalize(temp_dir.path()).unwrap()
    }

    /// The row shown while a directory is still being listed stands for no
    /// file: Enter, F3, F4 and Shift+Enter must not open it, and selection
    /// and batch operations must not pick it up.
    #[test]
    fn the_listing_placeholder_is_no_file() {
        let temp_dir = TempDir::new().unwrap();
        std::fs::create_dir(temp_dir.path().join("sub")).unwrap();
        std::fs::write(temp_dir.path().join("sub/a.txt"), "x").unwrap();
        let mut fm = FileManager::new_with_path(temp_dir.path().to_path_buf());
        // The listing is read on a worker thread; apply it as `tick()` would.
        fm.load_directory().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !fm.check_async_reload() {
            assert!(
                std::time::Instant::now() < deadline,
                "listing never arrived"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let sub = fm.find_entry_index("sub").unwrap();
        // Expanding lists on a worker thread too, and nothing applies that
        // listing here, so the placeholder stays.
        fm.expand_dir(sub);
        let placeholder = sub + 1;
        assert!(
            fm.tree_entry_at(placeholder)
                .is_some_and(|te| te.is_loading),
            "the listing is still pending"
        );

        fm.selected = placeholder;
        assert!(fm.enter().is_none());
        assert!(fm.edit_file().is_none());
        assert!(fm.view_file().is_none());
        assert!(fm.open_external().is_none());
        assert!(fm.get_selected_paths().is_empty());

        fm.select_all();
        assert!(!fm.selection.items.contains(&placeholder));
        assert!(fm
            .get_selected_paths()
            .iter()
            .all(|p| !p.ends_with("__loading__")));
    }

    fn wait_for_local_listing(fm: &mut FileManager) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !fm.check_async_reload() {
            assert!(
                std::time::Instant::now() < deadline,
                "listing never arrived"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn wait_for_vfs(fm: &mut FileManager) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while fm.vfs.has_pending_operation() {
            assert!(std::time::Instant::now() < deadline, "VFS never answered");
            fm.on_tick();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn names(fm: &FileManager) -> Vec<String> {
        (0..fm.visible_count())
            .filter_map(|i| fm.entry_at(i).map(|e| e.name.clone()))
            .collect()
    }

    /// A symlink in the selection keeps the copy on the text path: the file
    /// flavor resolves to the real path, so publishing it would deliver the
    /// target under the target's name. A plain selection takes the flavor.
    #[cfg(unix)]
    #[test]
    fn a_selected_symlink_keeps_the_copy_as_text() {
        let temp_dir = TempDir::new().unwrap();
        std::fs::write(temp_dir.path().join("real.txt"), "x").unwrap();
        std::fs::write(temp_dir.path().join("plain.txt"), "y").unwrap();
        std::os::unix::fs::symlink(
            temp_dir.path().join("real.txt"),
            temp_dir.path().join("link.txt"),
        )
        .unwrap();

        let mut fm = FileManager::new_with_path(temp_dir.path().to_path_buf());
        fm.load_directory().unwrap();
        wait_for_local_listing(&mut fm);

        // Cursor on a plain file: no link in play, the flavor is taken.
        fm.selected = fm.find_entry_index("plain.txt").unwrap();
        assert!(!fm.selection_has_symlink());

        // Cursor on the link: the panel sees it as a link, not its target.
        fm.selected = fm.find_entry_index("link.txt").unwrap();
        assert!(fm.selection_has_symlink(), "the cursor sits on a symlink");

        // A multi-selection holding the link reports it too.
        fm.clear_selection();
        fm.selection
            .select(fm.find_entry_index("plain.txt").unwrap());
        fm.selection
            .select(fm.find_entry_index("link.txt").unwrap());
        assert!(fm.selection_has_symlink());

        // Without the link, the multi-selection takes the flavor.
        fm.clear_selection();
        fm.selection
            .select(fm.find_entry_index("plain.txt").unwrap());
        fm.selection
            .select(fm.find_entry_index("real.txt").unwrap());
        assert!(!fm.selection_has_symlink());
    }

    /// `Cmd+V` reaches the panel as a bracketed paste, not as `Paste`: the
    /// terminal emulator answers the key and types the clipboard's text. When
    /// that text names real files the panel must offer the copy, and say
    /// which files — the count alone left the confirmation nameless.
    #[test]
    fn a_pasted_path_copies_the_file_and_names_it() {
        let temp_dir = TempDir::new().unwrap();
        let dir = canonical_temp_path(&temp_dir);
        std::fs::write(dir.join("report.pdf"), "x").unwrap();

        let mut fm = FileManager::new_with_path(dir.clone());
        fm.load_directory().unwrap();
        wait_for_local_listing(&mut fm);

        let pasted = dir.join("report.pdf").display().to_string();
        let result = fm.handle_command(PanelCommand::PasteText {
            text: pasted.clone(),
        });
        assert!(
            matches!(result, CommandResult::Handled(true)),
            "a pasted path must be taken as a file paste"
        );

        let (action, modal) = fm.modal_request.take().expect("a confirmation");
        match action {
            PendingAction::CopyPath {
                sources,
                target_directory,
                ..
            } => {
                assert_eq!(sources, [dir.join("report.pdf")]);
                assert_eq!(target_directory, Some(dir.clone()));
            }
            other => panic!("expected a copy, got {other:?}"),
        }

        let message = match modal {
            ActiveModal::Confirm(m) => m.message().to_string(),
            other => panic!("expected a confirmation, got {other:?}"),
        };
        assert!(
            message.contains("report.pdf"),
            "the confirmation must name the file, got {message:?}"
        );
    }

    /// Prose pasted into the panel is not a file: the key falls through
    /// untouched, so it stays available to whatever handles keys next.
    #[test]
    fn pasted_prose_is_not_taken_as_a_file() {
        let temp_dir = TempDir::new().unwrap();
        let dir = canonical_temp_path(&temp_dir);
        let mut fm = FileManager::new_with_path(dir.clone());
        fm.load_directory().unwrap();
        wait_for_local_listing(&mut fm);

        let result = fm.handle_command(PanelCommand::PasteText {
            text: "a sentence, not a path".to_string(),
        });
        assert!(
            matches!(result, CommandResult::Handled(false)),
            "prose must fall through"
        );
        assert!(fm.modal_request.is_none(), "no copy was offered");
    }

    /// A relative path resolves against the process working directory, not
    /// the panel's, so `exists()` can match some unrelated file and paste it
    /// under the wrong name. Only absolute paths are taken as files.
    #[test]
    fn a_pasted_relative_path_is_not_taken_as_a_file() {
        let temp_dir = TempDir::new().unwrap();
        let dir = canonical_temp_path(&temp_dir);

        let mut fm = FileManager::new_with_path(dir.clone());
        fm.load_directory().unwrap();
        wait_for_local_listing(&mut fm);

        // The bug this guards is silent unless the relative name really does
        // resolve somewhere: assert that it does, and that the panel still
        // refuses it. `Cargo.toml` exists in the crate directory the test
        // runs from, but not in the panel's directory.
        let relative = "Cargo.toml";
        assert!(
            std::path::Path::new(relative).exists(),
            "precondition: this name must resolve outside the panel"
        );
        assert!(!dir.join(relative).exists());

        let result = fm.handle_command(PanelCommand::PasteText {
            text: relative.to_string(),
        });
        assert!(
            matches!(result, CommandResult::Handled(false)),
            "a relative path must fall through rather than resolve elsewhere"
        );
        assert!(fm.modal_request.is_none(), "no copy was offered");
    }

    /// With the search bar open the same keystroke is query text, so the
    /// panel must not spend it on a copy even when it names a real file.
    #[test]
    fn a_pasted_path_is_left_to_the_open_search_bar() {
        let temp_dir = TempDir::new().unwrap();
        let dir = canonical_temp_path(&temp_dir);
        std::fs::write(dir.join("report.pdf"), "x").unwrap();

        let mut fm = FileManager::new_with_path(dir.clone());
        fm.load_directory().unwrap();
        wait_for_local_listing(&mut fm);

        fm.open_name_bar();
        let pasted = dir.join("report.pdf").display().to_string();
        let result = fm.handle_command(PanelCommand::PasteText { text: pasted });
        assert!(
            matches!(result, CommandResult::Handled(false)),
            "the search bar owns the paste"
        );
        assert!(fm.modal_request.is_none(), "no copy was offered");
    }

    /// Enter on an archive browses it like a directory; `..` at its root
    /// comes back with the cursor on the archive; nothing inside can be
    /// changed.
    #[test]
    fn an_archive_opens_like_a_directory_and_is_read_only() {
        use std::io::Write;
        let temp_dir = TempDir::new().unwrap();
        let archive = temp_dir.path().join("pack.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
        zip.start_file("docs/readme.md", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"hi").unwrap();
        zip.finish().unwrap();
        std::fs::write(temp_dir.path().join("other.txt"), "x").unwrap();

        let mut fm = FileManager::new_with_path(temp_dir.path().to_path_buf());
        fm.load_directory().unwrap();
        wait_for_local_listing(&mut fm);

        fm.selected = fm.find_entry_index("pack.zip").unwrap();
        assert!(fm.enter().is_none(), "an archive is not opened as a file");
        wait_for_vfs(&mut fm);
        assert!(fm.vfs.at_archive_root());
        assert_eq!(names(&fm), ["..", "docs"]);

        fm.execute_command(keyboard::FmCommand::DeleteFiles);
        assert!(
            matches!(
                fm.modal_request.take(),
                Some((PendingAction::VfsMessage, ActiveModal::InfoAction(_)))
            ),
            "a read-only notice, not a delete confirmation"
        );

        fm.selected = fm.find_entry_index("..").unwrap();
        assert!(fm.enter().is_none());
        assert!(fm.vfs.is_local());
        wait_for_local_listing(&mut fm);
        assert_eq!(fm.current_path, canonical_temp_path(&temp_dir));
        assert_eq!(
            fm.entry_at(fm.selected).map(|e| e.name.as_str()),
            Some("pack.zip"),
            "the cursor comes back to the archive"
        );
    }

    /// An archive inside an archive opens with Enter too, and `..` walks
    /// back out one level at a time, the cursor landing on what was left.
    #[test]
    fn a_nested_archive_opens_and_is_left_level_by_level() {
        use std::io::Write;
        fn zip_with(path: &std::path::Path, name: &str, data: &[u8]) {
            let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(data).unwrap();
            zip.finish().unwrap();
        }
        let temp_dir = TempDir::new().unwrap();
        let inner = temp_dir.path().join("inner.zip");
        zip_with(&inner, "deep/readme.md", b"hi");
        let outer = temp_dir.path().join("outer.zip");
        zip_with(&outer, "inner.zip", &std::fs::read(&inner).unwrap());
        std::fs::remove_file(&inner).unwrap();

        let mut fm = FileManager::new_with_path(temp_dir.path().to_path_buf());
        fm.load_directory().unwrap();
        wait_for_local_listing(&mut fm);

        fm.selected = fm.find_entry_index("outer.zip").unwrap();
        fm.enter();
        wait_for_vfs(&mut fm);
        assert_eq!(names(&fm), ["..", "inner.zip"]);

        fm.selected = fm.find_entry_index("inner.zip").unwrap();
        fm.enter();
        wait_for_vfs(&mut fm);
        assert_eq!(names(&fm), ["..", "deep"]);
        assert!(fm.vfs.current_path().container().unwrap().is_archive());

        fm.selected = fm.find_entry_index("..").unwrap();
        fm.enter();
        wait_for_vfs(&mut fm);
        assert_eq!(names(&fm), ["..", "inner.zip"]);
        assert_eq!(
            fm.entry_at(fm.selected).map(|e| e.name.as_str()),
            Some("inner.zip")
        );

        fm.selected = fm.find_entry_index("..").unwrap();
        fm.enter();
        wait_for_local_listing(&mut fm);
        assert!(fm.vfs.is_local());
        assert_eq!(
            fm.entry_at(fm.selected).map(|e| e.name.as_str()),
            Some("outer.zip")
        );
    }

    /// An encrypted archive asks for its password, asks again after a wrong
    /// one, and opens with the right one.
    #[test]
    fn an_encrypted_archive_asks_for_its_password() {
        use std::io::Write;
        let temp_dir = TempDir::new().unwrap();
        let archive = temp_dir.path().join("locked.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
        zip.start_file(
            "secret.txt",
            zip::write::SimpleFileOptions::default()
                .with_aes_encryption(zip::AesMode::Aes256, "hunter2"),
        )
        .unwrap();
        zip.write_all(b"x").unwrap();
        zip.finish().unwrap();

        let mut fm = FileManager::new_with_path(temp_dir.path().to_path_buf());
        fm.load_directory().unwrap();
        wait_for_local_listing(&mut fm);
        let asked = |fm: &mut FileManager| match fm.modal_request.take() {
            Some((PendingAction::ArchivePassword { archive }, ActiveModal::Input(_))) => archive,
            other => panic!(
                "expected a password prompt, got {:?}",
                other.map(|(a, _)| a)
            ),
        };

        fm.selected = fm.find_entry_index("locked.zip").unwrap();
        fm.enter();
        wait_for_vfs(&mut fm);
        fm.on_tick();
        let root = asked(&mut fm);
        assert!(fm.vfs.is_local(), "the panel stays in the directory");

        fm.open_archive_with_password(root, "wrong".to_string());
        wait_for_vfs(&mut fm);
        fm.on_tick();
        let root = asked(&mut fm);

        fm.open_archive_with_password(root, "hunter2".to_string());
        wait_for_vfs(&mut fm);
        assert_eq!(names(&fm), ["..", "secret.txt"]);
    }

    /// P offers an archive next to the selection in every writable format;
    /// inside an archive it is refused instead.
    #[test]
    fn pack_prompts_for_an_archive_next_to_the_selection() {
        let temp_dir = TempDir::new().unwrap();
        std::fs::create_dir(temp_dir.path().join("docs")).unwrap();
        std::fs::write(temp_dir.path().join("report.pdf"), "x").unwrap();
        let mut fm = FileManager::new_with_path(temp_dir.path().to_path_buf());
        fm.load_directory().unwrap();
        wait_for_local_listing(&mut fm);
        let dir = canonical_temp_path(&temp_dir);

        fm.selected = fm.find_entry_index("report.pdf").unwrap();
        fm.execute_command(keyboard::FmCommand::Pack);
        match fm.modal_request.take() {
            Some((PendingAction::PackPaths { sources }, ActiveModal::EditableSelect(modal))) => {
                assert_eq!(sources, [dir.join("report.pdf")]);
                assert_eq!(modal.value(), dir.join("report.zip").display().to_string());
            }
            other => panic!("expected the pack prompt, got {:?}", other.map(|(a, _)| a)),
        }

        fm.selected = fm.find_entry_index("docs").unwrap();
        fm.execute_command(keyboard::FmCommand::Pack);
        match fm.modal_request.take() {
            Some((PendingAction::PackPaths { .. }, ActiveModal::EditableSelect(modal))) => {
                assert_eq!(modal.value(), dir.join("docs.zip").display().to_string());
            }
            other => panic!("expected the pack prompt, got {:?}", other.map(|(a, _)| a)),
        }

        fm.vfs.set_path(termide_vfs::VfsPath::archive(
            termide_vfs::VfsPath::local(dir.join("a.zip")),
            "/",
        ));
        fm.execute_command(keyboard::FmCommand::Pack);
        assert!(matches!(
            fm.modal_request.take(),
            Some((PendingAction::VfsMessage, ActiveModal::InfoAction(_)))
        ));
    }

    /// A scrollbar drag must not be undone by the next render: the panel pulls
    /// `scroll_offset` back toward `selected` while drawing, so the command has
    /// to move the cursor into the new viewport as wheel scrolling does.
    #[test]
    fn set_scroll_offset_survives_the_next_render() {
        use ratatui::buffer::Buffer;

        let temp_dir = TempDir::new().unwrap();
        for i in 0..80 {
            std::fs::write(temp_dir.path().join(format!("f{i:03}.txt")), "x").unwrap();
        }
        let mut fm = FileManager::new_with_path(temp_dir.path().to_path_buf());
        let colors = termide_core::ThemeColors::from(&fm.cached_theme);
        let panel_config = termide_core::PanelConfig {
            tab_size: 4,
            word_wrap: false,
            show_line_numbers: false,
            show_hidden_files: false,
        };
        let area = Rect::new(0, 0, 30, 10);
        let ctx = RenderContext {
            theme: &colors,
            config: &panel_config,
            is_focused: true,
            panel_index: 0,
            terminal_width: 30,
            terminal_height: 12,
            border_right_x: Some(29),
            border_bottom_y: Some(11),
        };
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 12));
        fm.render(area, &mut buf, &ctx);

        fm.handle_command(PanelCommand::SetScrollOffset {
            axis: termide_core::ScrollAxis::Vertical,
            offset: 40,
        });
        assert_eq!(fm.scroll_offset, 40);
        assert!(
            fm.selected >= 40 && fm.selected < 40 + fm.visible_height.max(1),
            "cursor left outside the new viewport: selected={}, offset=40",
            fm.selected
        );

        fm.render(area, &mut buf, &ctx);
        assert_eq!(
            fm.scroll_offset, 40,
            "render pulled the scroll back to the cursor"
        );
    }

    /// Creating on an expanded directory must land inside it: its children
    /// are what the tree shows under the cursor. A collapsed directory at
    /// the same level keeps the "alongside the cursor" rule.
    #[test]
    fn create_directory_on_expanded_dir_lands_inside_it() {
        let (mut fm, temp_dir) = create_file_manager_in_temp();
        std::fs::create_dir(temp_dir.path().join("sub")).unwrap();
        std::fs::create_dir(temp_dir.path().join("other")).unwrap();
        // The listing is read on a worker thread; apply it before looking
        // rows up, as `tick()` would.
        let reload = |fm: &mut FileManager| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !fm.check_async_reload() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "listing never arrived"
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };
        fm.load_directory().unwrap();
        reload(&mut fm);
        let vis_of = |fm: &FileManager, name: &str| {
            (0..fm.visible_indices.len())
                .find(|&i| fm.tree_entry_at(i).unwrap().file_entry.name == name)
                .unwrap()
        };

        let sub = vis_of(&fm, "sub");
        fm.expand_dir(sub);
        fm.selected = sub;
        fm.create_directory("inside".to_string()).unwrap();
        assert!(canonical_temp_path(&temp_dir).join("sub/inside").is_dir());
        reload(&mut fm);

        fm.selected = vis_of(&fm, "other");
        fm.create_directory("beside".to_string()).unwrap();
        assert!(canonical_temp_path(&temp_dir).join("beside").is_dir());
    }

    #[test]
    fn test_file_manager_new() {
        let (fm, temp_dir) = create_file_manager_in_temp();
        assert_eq!(fm.current_path(), canonical_temp_path(&temp_dir));
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
            assert_eq!(current_path, canonical_temp_path(&temp_dir));
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
