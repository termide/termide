//! File manager panel for termide.
//!
//! Provides a smart file manager with git integration, drag selection, and file operations.

mod file_info;
mod navigation;
mod operations;
mod rendering;
mod selection;
mod utils;

pub use file_info::FileInfo;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{buffer::Buffer, layout::Rect, prelude::Widget, widgets::Paragraph};
use std::any::Any;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc;

use termide_config::{constants, Config, FileManagerSettings};
use termide_core::{CommandResult, Panel, PanelCommand, PanelEvent, RenderContext, SessionPanel};
use termide_git::{get_git_status, GitStatus, GitStatusCache};
use termide_modal::{ActiveModal, ConfirmModal, InputModal};
use termide_state::{DirSizeResult, PendingAction};
use termide_theme::Theme;
use termide_ui::{clipboard, path_utils};

#[derive(Debug, Clone, Copy, PartialEq)]
enum DragMode {
    Select, // Shift+drag - selection
    Toggle, // Ctrl+drag - toggle selection
}

/// Smart file manager with advanced features
pub struct FileManager {
    current_path: PathBuf,
    entries: Vec<FileEntry>,
    selected: usize,
    scroll_offset: usize,
    /// Last displayed title (cached for [X] clicks)
    display_title: String,
    /// Modal window request (action, modal)
    modal_request: Option<(PendingAction, ActiveModal)>,
    /// Visible area height (updated during rendering)
    visible_height: usize,
    /// Time of last click for double-click detection
    last_click_time: Option<std::time::Instant>,
    /// Index of element that was last clicked
    last_click_index: Option<usize>,
    /// Set of selected items (indices)
    selected_items: HashSet<usize>,
    /// Git status cache for the current directory
    git_status_cache: Option<GitStatusCache>,
    /// Channel receiver for directory size calculation results (needs to be passed to AppState)
    pub dir_size_receiver: Option<mpsc::Receiver<DirSizeResult>>,
    /// Starting index for drag selection
    drag_start_index: Option<usize>,
    /// Drag mode (Shift/Ctrl)
    drag_mode: Option<DragMode>,
    /// Set of items already processed during current drag (to avoid re-toggling)
    dragged_items: HashSet<usize>,
    /// Name of directory we came from (for cursor restoration when going up)
    previous_dir_name: Option<String>,
    /// Currently watched root path (repo_root for git, or directory itself for non-git)
    /// Used for reference counting when navigating between directories
    watched_root: Option<PathBuf>,
    /// Whether the watched root is a git repository (cached to avoid repeated filesystem checks)
    is_watched_root_git_repo: bool,
    /// Last reload time for debouncing rapid reload_directory() calls
    last_reload_time: Option<std::time::Instant>,
    /// Cached theme for rendering
    cached_theme: Theme,
    /// Cached config for rendering
    cached_config: FileManagerSettings,
}

#[derive(Debug, Clone)]
pub(crate) struct FileEntry {
    pub name: String,
    pub is_dir: bool,
    #[allow(dead_code)]
    pub is_hidden: bool,
    pub is_symlink: bool,
    pub is_executable: bool,
    pub is_readonly: bool,
    pub git_status: GitStatus,
    pub size: Option<u64>,
    pub modified: Option<std::time::SystemTime>,
}

impl FileManager {
    /// Create a new smart file manager
    pub fn new() -> Self {
        let current_path = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        Self::new_with_path(current_path)
    }

    /// Create a new smart file manager with the specified path
    pub fn new_with_path(current_path: PathBuf) -> Self {
        let display_title = current_path.display().to_string();
        let mut fm = Self {
            current_path,
            entries: Vec::new(),
            selected: 0,
            scroll_offset: 0,
            display_title,
            modal_request: None,
            visible_height: 10, // Default value, will be updated during rendering
            last_click_time: None,
            last_click_index: None,
            selected_items: HashSet::new(),
            git_status_cache: None,
            dir_size_receiver: None,
            drag_start_index: None,
            drag_mode: None,
            dragged_items: HashSet::new(),
            previous_dir_name: None,
            watched_root: None,
            is_watched_root_git_repo: false,
            last_reload_time: None,
            cached_theme: Theme::default(),
            cached_config: FileManagerSettings::default(),
        };
        let _ = fm.load_directory();
        fm
    }

    /// Get the current directory
    pub fn get_current_directory(&self) -> PathBuf {
        self.current_path.clone()
    }

    /// Get the currently watched root path
    pub fn watched_root(&self) -> Option<&PathBuf> {
        self.watched_root.as_ref()
    }

    /// Set the watched root path and whether it's a git repository
    pub fn set_watched_root(&mut self, root: Option<PathBuf>, is_git_repo: bool) {
        self.watched_root = root;
        self.is_watched_root_git_repo = is_git_repo;
    }

    /// Check if the watched root is a git repository (cached value)
    pub fn is_watched_root_git_repo(&self) -> bool {
        self.is_watched_root_git_repo
    }

    /// Check if absolute path is in a gitignored directory
    /// Uses cached git_status_cache to avoid spawning git processes
    pub fn is_path_ignored(&self, absolute_path: &std::path::Path) -> bool {
        // Need repo root (watched_root) and git_status_cache
        let repo_root = match self.watched_root.as_ref() {
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
        self.watched_root.take()
    }

    /// Navigate to a specific directory
    pub fn navigate_to(&mut self, path: PathBuf) -> Result<()> {
        if path.is_dir() {
            self.current_path = path;
            self.load_directory()
        } else if let Some(parent) = path.parent() {
            // If path is a file, navigate to its parent directory
            self.current_path = parent.to_path_buf();
            self.load_directory()
        } else {
            Ok(())
        }
    }

    /// Load the contents of the current directory
    pub fn load_directory(&mut self) -> Result<()> {
        // Invalidate watched_root when navigating to a new directory
        // This triggers re-registration with fs_watcher in check_fs_update()
        self.watched_root = None;
        self.is_watched_root_git_repo = false;
        self.load_directory_inner(false)
    }

    /// Reload directory preserving selection (with debounce to prevent rapid reloads)
    pub fn reload_directory(&mut self) -> Result<()> {
        const RELOAD_DEBOUNCE_MS: u128 = 300;

        // Debounce: skip if last reload was too recent
        let now = std::time::Instant::now();
        if let Some(last) = self.last_reload_time {
            if now.duration_since(last).as_millis() < RELOAD_DEBOUNCE_MS {
                return Ok(()); // Skip rapid reloads
            }
        }
        self.last_reload_time = Some(now);

        self.load_directory_inner(true)
    }

    /// Internal method to load directory with optional selection preservation
    fn load_directory_inner(&mut self, preserve_selection: bool) -> Result<()> {
        // Save current file name and index to restore position
        // Use previous_dir_name if navigating up, otherwise use current selection
        let current_name = self
            .previous_dir_name
            .take()
            .or_else(|| self.entries.get(self.selected).map(|e| e.name.clone()));
        let previous_index = self.selected;
        let previous_scroll_offset = self.scroll_offset;

        // Save names of selected files if we need to restore selection
        let selected_names: HashSet<String> = if preserve_selection {
            self.selected_items
                .iter()
                .filter_map(|&idx| self.entries.get(idx).map(|e| e.name.clone()))
                .collect()
        } else {
            HashSet::new()
        };

        self.entries.clear();
        self.selected = 0;
        self.scroll_offset = 0;
        // Clear selection indices (will restore by names if preserve_selection)
        self.selected_items.clear();
        // Clear drag state
        self.drag_start_index = None;
        self.drag_mode = None;
        self.dragged_items.clear();

        // Update displayed title (will be truncated during rendering if needed)
        self.display_title = self.current_path.display().to_string();

        // Load git statuses for the current directory
        self.git_status_cache = get_git_status(&self.current_path);

        // Add parent directory if not at root
        if self.current_path.parent().is_some() {
            self.entries.push(FileEntry {
                name: "..".to_string(),
                is_dir: true,
                is_hidden: false,
                is_symlink: false,
                is_executable: false,
                is_readonly: false,
                git_status: GitStatus::Unmodified,
                size: None,
                modified: None,
            });
        }

        // Read directory contents
        if let Ok(read_dir) = fs::read_dir(&self.current_path) {
            for entry in read_dir.flatten() {
                if let Ok(metadata) = entry.metadata() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let is_hidden = name.starts_with('.');

                    // Determine git status for this entry
                    let git_status = if metadata.is_dir() {
                        // For directories: check recursively for nested changes
                        self.git_status_cache
                            .as_ref()
                            .map(|cache| cache.get_directory_status(&name))
                            .unwrap_or(GitStatus::Unmodified)
                    } else {
                        // For files: use direct status
                        self.git_status_cache
                            .as_ref()
                            .map(|cache| cache.get_status(&name))
                            .unwrap_or(GitStatus::Unmodified)
                    };

                    // Check if this is a symlink (use symlink_metadata to not follow links)
                    let is_symlink = if let Ok(link_metadata) = fs::symlink_metadata(entry.path()) {
                        link_metadata.is_symlink()
                    } else {
                        false
                    };

                    // Check if file is executable (Unix permissions)
                    #[cfg(unix)]
                    let is_executable = {
                        use std::os::unix::fs::PermissionsExt;
                        metadata.permissions().mode() & 0o111 != 0
                    };
                    #[cfg(not(unix))]
                    let is_executable = false;

                    // Check if file is read-only (Unix permissions)
                    #[cfg(unix)]
                    let is_readonly = {
                        use std::os::unix::fs::PermissionsExt;
                        let mode = metadata.permissions().mode();
                        (mode & 0o200) == 0 // owner write bit
                    };
                    #[cfg(not(unix))]
                    let is_readonly = metadata.permissions().readonly();

                    // Get size (files only) and modification time
                    let size = if metadata.is_file() {
                        Some(metadata.len())
                    } else {
                        None
                    };
                    let modified = metadata.modified().ok();

                    self.entries.push(FileEntry {
                        name,
                        is_dir: metadata.is_dir(),
                        is_hidden,
                        is_symlink,
                        is_executable,
                        is_readonly,
                        git_status,
                        size,
                        modified,
                    });
                }
            }
        } else {
            log::warn!("Failed to read directory: {}", self.current_path.display());
        }

        // Add virtual entries for deleted files (tracked by git but removed from filesystem)
        if let Some(cache) = &self.git_status_cache {
            for deleted_name in cache.get_deleted_files() {
                // Skip if already in entries (shouldn't happen, but safety check)
                if self.entries.iter().any(|e| e.name == deleted_name) {
                    continue;
                }
                self.entries.push(FileEntry {
                    name: deleted_name,
                    is_dir: false, // Assume file (git doesn't track empty dirs)
                    is_hidden: false,
                    is_symlink: false,
                    is_executable: false,
                    is_readonly: false, // Don't show "R" attribute for deleted
                    git_status: GitStatus::Deleted,
                    size: None,
                    modified: None,
                });
            }
        }

        // Sort: directories first, then files
        self.entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        });

        // Restore selection by file names
        if !selected_names.is_empty() {
            for (idx, entry) in self.entries.iter().enumerate() {
                if selected_names.contains(&entry.name) {
                    self.selected_items.insert(idx);
                }
            }
        }

        // Restore cursor position
        if let Some(name) = current_name {
            if let Some(pos) = self.entries.iter().position(|e| e.name == name) {
                // Found file by name - restore to its position
                self.selected = pos;
            } else if !self.entries.is_empty() {
                // File not found (deleted) - use previous index or last available
                self.selected = previous_index.min(self.entries.len() - 1);
            }

            // Restore scroll_offset using real visible_height
            if self.visible_height > 0 {
                // If all items fit on screen - no scroll needed
                if self.entries.len() <= self.visible_height {
                    self.scroll_offset = 0;
                } else {
                    // Restore previous offset if still valid
                    let max_scroll = self.entries.len().saturating_sub(self.visible_height);
                    self.scroll_offset = previous_scroll_offset.min(max_scroll);
                }
                // Ensure cursor is visible
                self.adjust_scroll_offset(self.visible_height);
            }
            // If visible_height == 0, render() will recalculate on first draw
        }

        Ok(())
    }

    /// Get current directory path
    pub fn current_path(&self) -> &std::path::Path {
        &self.current_path
    }

    /// Enter directory or open file
    /// Returns `Some(PanelEvent::OpenFile)` if a file should be opened
    fn enter(&mut self) -> Option<PanelEvent> {
        if let Some(entry) = self.entries.get(self.selected) {
            // Prohibit operations on deleted files
            if entry.git_status == GitStatus::Deleted {
                return None;
            }

            if entry.name == ".." {
                // Save current directory name before going up
                if let Some(dir_name) = self.current_path.file_name() {
                    self.previous_dir_name = Some(dir_name.to_string_lossy().to_string());
                }
                if let Some(parent) = self.current_path.parent() {
                    self.current_path = parent.to_path_buf();
                    let _ = self.load_directory();
                }
            } else if entry.is_dir {
                self.previous_dir_name = None; // Clear when going down
                self.current_path.push(&entry.name);
                let _ = self.load_directory();
            } else {
                // This is a file - emit event to open in editor
                let file_path = self.current_path.join(&entry.name);
                return Some(PanelEvent::OpenFile(file_path));
            }
        }
        None
    }

    /// Open file for editing (F4)
    /// Returns `Some(PanelEvent::OpenFile)` if a file should be opened
    fn edit_file(&mut self) -> Option<PanelEvent> {
        if let Some(entry) = self.entries.get(self.selected) {
            // Prohibit operations on deleted files
            if entry.git_status == GitStatus::Deleted {
                return None;
            }

            // Check that this is a file, not a directory and not ".."
            if !entry.is_dir && entry.name != ".." {
                let file_path = self.current_path.join(&entry.name);
                return Some(PanelEvent::OpenFile(file_path));
            }
        }
        None
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
        self.display_title.clone()
    }

    fn prepare_render(&mut self, theme: &termide_theme::Theme, config: &Config) {
        self.cached_theme = *theme;
        self.cached_config = config.file_manager.clone();
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, ctx: &RenderContext) {
        let _ = ctx; // Use ctx in future for theme/config
                     // Automatically update scroll offset
                     // area is already the inner content area (accordion drew outer border)
        let content_height = area.height as usize;
        self.visible_height = content_height; // Save for use in handle_key

        if self.selected >= self.scroll_offset + content_height {
            self.scroll_offset = self.selected - content_height + 1;
        } else if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        }

        // Get display path taking into account panel width
        self.display_title = self.get_display_title(area.width);

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
    }

    fn handle_key(&mut self, key: KeyEvent) -> Vec<PanelEvent> {
        // Translate Cyrillic to Latin for hotkeys
        let key = termide_keyboard::translate_hotkey(key);

        // Collect events to return
        let mut events = Vec::new();

        match (key.code, key.modifiers) {
            // Ctrl+A - select all
            (KeyCode::Char('a'), KeyModifiers::CONTROL) => {
                self.select_all();
            }
            // Ctrl+R - refresh file list
            (KeyCode::Char('r'), KeyModifiers::CONTROL) => {
                let _ = self.reload_directory();
            }
            // Insert - toggle selection of current item and move down
            (KeyCode::Insert, KeyModifiers::NONE) => {
                self.toggle_selection();
                self.move_down();
            }
            // Space - show file information
            (KeyCode::Char(' '), KeyModifiers::NONE) => {
                self.show_file_info();
            }
            // Shift+Down - select down
            (KeyCode::Down, KeyModifiers::SHIFT) => {
                self.move_down_with_selection();
            }
            // Shift+Up - select up
            (KeyCode::Up, KeyModifiers::SHIFT) => {
                self.move_up_with_selection();
            }
            // Shift+PageDown - select page down
            (KeyCode::PageDown, KeyModifiers::SHIFT) => {
                self.page_down_with_selection();
            }
            // Shift+PageUp - select page up
            (KeyCode::PageUp, KeyModifiers::SHIFT) => {
                self.page_up_with_selection();
            }
            // Shift+Home - select to beginning
            (KeyCode::Home, KeyModifiers::SHIFT) => {
                self.select_to_home();
            }
            // Shift+End - select to end
            (KeyCode::End, KeyModifiers::SHIFT) => {
                self.select_to_end();
            }
            // Ctrl+Down - toggle selection down
            (KeyCode::Down, KeyModifiers::CONTROL) => {
                self.move_down_with_toggle();
            }
            // Ctrl+Up - toggle selection up
            (KeyCode::Up, KeyModifiers::CONTROL) => {
                self.move_up_with_toggle();
            }
            // Ctrl+PageDown - toggle selection page down
            (KeyCode::PageDown, KeyModifiers::CONTROL) => {
                self.page_down_with_toggle();
            }
            // Ctrl+PageUp - toggle selection page up
            (KeyCode::PageUp, KeyModifiers::CONTROL) => {
                self.page_up_with_toggle();
            }
            // Regular keys - move without clearing selection
            (KeyCode::Down, KeyModifiers::NONE) => {
                self.move_down();
            }
            (KeyCode::Up, KeyModifiers::NONE) => {
                self.move_up();
            }
            // Escape - clear selection
            (KeyCode::Esc, KeyModifiers::NONE) => {
                self.selected_items.clear();
            }
            (KeyCode::Enter, _) => {
                if let Some(event) = self.enter() {
                    events.push(event);
                }
            }
            (KeyCode::Backspace, _) => {
                // Return to parent directory
                // Save current directory name before going up
                if let Some(dir_name) = self.current_path.file_name() {
                    self.previous_dir_name = Some(dir_name.to_string_lossy().to_string());
                }
                if let Some(parent) = self.current_path.parent() {
                    self.current_path = parent.to_path_buf();
                    let _ = self.load_directory();
                }
            }
            (KeyCode::PageUp, KeyModifiers::NONE) => {
                // Scroll up by visible area
                self.selected = self.selected.saturating_sub(self.visible_height);
            }
            (KeyCode::PageDown, KeyModifiers::NONE) => {
                // Scroll down by visible area
                let max_index = self.entries.len().saturating_sub(1);
                self.selected = (self.selected + self.visible_height).min(max_index);
            }
            (KeyCode::Home, KeyModifiers::NONE) => {
                // Go to beginning of list
                self.selected = 0;
                self.scroll_offset = 0;
            }
            (KeyCode::End, KeyModifiers::NONE) => {
                // Go to end of list
                self.selected = self.entries.len().saturating_sub(1);
            }
            (KeyCode::Char('~'), _) => {
                // Go to home directory
                if let Some(home) = dirs::home_dir() {
                    self.current_path = home;
                    let _ = self.load_directory();
                }
            }
            (KeyCode::Char('f'), _) | (KeyCode::Char('F'), _) => {
                // Create new file - open InputModal
                let t = termide_i18n::t();
                let modal = InputModal::new(t.modal_create_file_title(), "");
                let action = PendingAction::CreateFile {
                    panel_index: 0, // will be updated in app.rs
                    directory: self.current_path.clone(),
                };
                self.modal_request = Some((action, ActiveModal::Input(Box::new(modal))));
            }
            (KeyCode::Char('d'), _) | (KeyCode::Char('D'), _) | (KeyCode::F(7), _) => {
                // Create new directory - open InputModal
                let t = termide_i18n::t();
                let modal = InputModal::new(t.modal_create_dir_title(), "");
                let action = PendingAction::CreateDirectory {
                    panel_index: 0, // will be updated in app.rs
                    directory: self.current_path.clone(),
                };
                self.modal_request = Some((action, ActiveModal::Input(Box::new(modal))));
            }
            (KeyCode::Delete, _) | (KeyCode::F(8), _) => {
                // Delete selected files/directories - open ConfirmModal
                let paths = self.get_selected_paths();
                if paths.is_empty() {
                    return vec![];
                }

                let t = termide_i18n::t();
                let title = if paths.len() == 1 {
                    let file_name = path_utils::get_file_name_str(&paths[0]);
                    t.modal_delete_single_title(file_name)
                } else {
                    t.modal_delete_multiple_title(paths.len())
                };

                let modal = ConfirmModal::new(&title, "");
                let action = PendingAction::DeletePath {
                    panel_index: 0, // will be updated in app.rs
                    paths,
                };
                self.modal_request = Some((action, ActiveModal::Confirm(Box::new(modal))));
            }
            (KeyCode::F(4), _) => {
                // Open selected file for editing
                if let Some(event) = self.edit_file() {
                    events.push(event);
                }
            }
            // Ctrl+C - copy selected files to clipboard
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
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
            // Ctrl+X - cut selected files to clipboard
            (KeyCode::Char('x'), KeyModifiers::CONTROL) => {
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
            // Ctrl+V - paste files from clipboard
            (KeyCode::Char('v'), KeyModifiers::CONTROL) => {
                if let Some(text) = clipboard::paste() {
                    // Split text by newlines and convert to paths
                    let files: Vec<std::path::PathBuf> = text
                        .lines()
                        .filter(|line| !line.is_empty())
                        .map(std::path::PathBuf::from)
                        .filter(|path| path.exists()) // Only existing paths
                        .collect();

                    if !files.is_empty() {
                        // Create confirmation modal
                        let t = termide_i18n::t();
                        let message = t.fm_paste_confirm(
                            files.len(),
                            "Copy",
                            &self.current_path.display().to_string(),
                        );

                        let action = PendingAction::CopyPath {
                            panel_index: 0,
                            sources: files,
                            target_directory: Some(self.current_path.clone()),
                        };

                        let modal = ConfirmModal::new("Confirm", &message);
                        self.modal_request = Some((action, ActiveModal::Confirm(Box::new(modal))));
                    }
                }
            }
            (KeyCode::Char('c'), _) | (KeyCode::Char('C'), _) | (KeyCode::F(5), _) => {
                // Copy selected files/directories
                let paths = self.get_selected_paths();
                if paths.is_empty() {
                    return vec![];
                }

                // Default - current directory
                let default_dest = format!("{}/", self.current_path.display());

                let t = termide_i18n::t();
                let message = if paths.len() == 1 {
                    let name = path_utils::get_file_name_str(&paths[0]);
                    t.fm_copy_prompt(name)
                } else {
                    format!("Copy {} items to:", paths.len())
                };

                let modal = InputModal::with_default("Copy", &message, &default_dest);
                let action = PendingAction::CopyPath {
                    panel_index: 0, // will be updated in app.rs
                    sources: paths,
                    target_directory: None, // will be set in app.rs
                };
                self.modal_request = Some((action, ActiveModal::Input(Box::new(modal))));
            }
            (KeyCode::Char('m'), _) | (KeyCode::Char('M'), _) | (KeyCode::F(6), _) => {
                // Move selected files/directories
                let paths = self.get_selected_paths();
                if paths.is_empty() {
                    return vec![];
                }

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
                    panel_index: 0, // will be updated in app.rs
                    sources: paths,
                    target_directory: None, // will be set in app.rs
                };
                self.modal_request = Some((action, ActiveModal::Input(Box::new(modal))));
            }
            // Tab - go to next panel
            (KeyCode::Tab, KeyModifiers::NONE) => {
                // Use dummy ConfirmModal that won't be shown
                let modal = ConfirmModal::new("", "");
                self.modal_request = Some((
                    PendingAction::NextPanel,
                    ActiveModal::Confirm(Box::new(modal)),
                ));
            }
            // Shift+Tab - go to previous panel
            (KeyCode::BackTab, _) => {
                let modal = ConfirmModal::new("", "");
                self.modal_request = Some((
                    PendingAction::PrevPanel,
                    ActiveModal::Confirm(Box::new(modal)),
                ));
            }
            _ => {}
        }
        events
    }

    fn handle_mouse(
        &mut self,
        mouse: crossterm::event::MouseEvent,
        panel_area: Rect,
    ) -> Vec<PanelEvent> {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};

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
                let max_scroll = self.entries.len().saturating_sub(visible_height);
                self.scroll_offset = (self.scroll_offset + 3).min(max_scroll);
                // Keep selected in visible area so render doesn't reset scroll
                if self.selected < self.scroll_offset {
                    self.selected = self.scroll_offset;
                }
                return vec![];
            }
            MouseEventKind::Up(MouseButton::Left) => {
                // End drag - handle this ALWAYS, even if outside panel
                self.drag_start_index = None;
                self.drag_mode = None;
                self.dragged_items.clear();
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

                if clicked_index < self.entries.len() {
                    // Check modifiers
                    if mouse.modifiers.contains(KeyModifiers::SHIFT) {
                        // Shift+click - select range from selected to clicked_index
                        let start = self.selected.min(clicked_index);
                        let end = self.selected.max(clicked_index);
                        self.dragged_items.clear();
                        for i in start..=end {
                            self.selected_items.insert(i);
                            self.dragged_items.insert(i);
                        }
                        self.selected = clicked_index;
                        self.drag_start_index = Some(clicked_index);
                        self.drag_mode = Some(DragMode::Select);
                    } else if mouse.modifiers.contains(KeyModifiers::CONTROL) {
                        // Ctrl+click - toggle selection on clicked element
                        if self.selected_items.contains(&clicked_index) {
                            self.selected_items.remove(&clicked_index);
                        } else {
                            self.selected_items.insert(clicked_index);
                        }
                        self.selected = clicked_index;
                        self.drag_start_index = Some(clicked_index);
                        self.drag_mode = Some(DragMode::Toggle);
                        // Track this item as already processed during drag
                        self.dragged_items.clear();
                        self.dragged_items.insert(clicked_index);
                    } else {
                        // Check for double click
                        let now = std::time::Instant::now();
                        let is_double_click = if let (Some(last_time), Some(last_index)) =
                            (self.last_click_time, self.last_click_index)
                        {
                            // Double click if less than DOUBLE_CLICK_INTERVAL_MS passed and clicked on same item
                            now.duration_since(last_time).as_millis()
                                < constants::DOUBLE_CLICK_INTERVAL_MS
                                && last_index == clicked_index
                        } else {
                            false
                        };

                        if is_double_click {
                            // Double click - open file/directory
                            self.selected = clicked_index;
                            let event = self.enter();
                            // Reset click state
                            self.last_click_time = None;
                            self.last_click_index = None;
                            // Return event if file was opened
                            if let Some(e) = event {
                                return vec![e];
                            }
                        } else {
                            // Single click - select item
                            self.selected = clicked_index;
                            // Save time and index for double-click detection
                            self.last_click_time = Some(now);
                            self.last_click_index = Some(clicked_index);
                        }
                        self.drag_start_index = None;
                        self.drag_mode = None;
                    }
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                // Handle drag only if there's a drag_mode
                if let Some(drag_mode) = self.drag_mode {
                    let relative_row = (mouse.row - inner_area.y) as usize;
                    let current_index = self.scroll_offset + relative_row;

                    if current_index < self.entries.len() {
                        match drag_mode {
                            DragMode::Select => {
                                // Shift+drag - select current item if not already processed
                                if !self.dragged_items.contains(&current_index) {
                                    self.selected_items.insert(current_index);
                                    self.dragged_items.insert(current_index);
                                }
                            }
                            DragMode::Toggle => {
                                // Ctrl+drag - toggle current item if not already processed
                                if !self.dragged_items.contains(&current_index) {
                                    if self.selected_items.contains(&current_index) {
                                        self.selected_items.remove(&current_index);
                                    } else {
                                        self.selected_items.insert(current_index);
                                    }
                                    self.dragged_items.insert(current_index);
                                }
                            }
                        }
                        self.selected = current_index;
                    }
                }
            }
            _ => {}
        }

        vec![]
    }

    fn reload(&mut self) -> anyhow::Result<()> {
        // Reload directory contents (preserving selection)
        self.reload_directory()
    }

    fn handle_command(&mut self, cmd: PanelCommand<'_>) -> CommandResult {
        match cmd {
            PanelCommand::GetFsWatchInfo => CommandResult::FsWatchInfo {
                watched_root: self.watched_root.clone(),
                current_path: self.current_path.clone(),
                is_git_repo: self.is_watched_root_git_repo,
            },
            PanelCommand::SetFsWatchRoot { root, is_git_repo } => {
                self.set_watched_root(root, is_git_repo);
                CommandResult::None
            }
            PanelCommand::OnFsUpdate { changed_path } => {
                let current = self.current_path();

                // For git repos: reload on any change within current directory tree
                // (needed for git status color updates)
                // For non-git dirs: reload only for direct children
                let should_reload = if self.is_watched_root_git_repo {
                    // Git repo: any change within current directory tree updates git status
                    // But skip gitignored paths (like target/) to avoid unnecessary reloads
                    changed_path.starts_with(current) && !self.is_path_ignored(changed_path)
                } else {
                    // Non-git: only direct children or current dir itself
                    changed_path.parent() == Some(current) || changed_path == current
                };

                if should_reload {
                    let _ = self.reload_directory();
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
            // Commands not applicable to FileManager
            PanelCommand::GetRepoRoot
            | PanelCommand::OnGitUpdate { .. }
            | PanelCommand::CheckPendingGitDiff
            | PanelCommand::CheckGitDiffReceiver
            | PanelCommand::CheckExternalModification
            | PanelCommand::Resize { .. }
            | PanelCommand::GetModificationStatus
            | PanelCommand::Save
            | PanelCommand::CloseWithoutSaving => CommandResult::None,
        }
    }

    fn needs_close_confirmation(&self) -> Option<String> {
        // FileManager doesn't store critical state by itself
        // Pending batch operations are checked in has_panels_requiring_confirmation()
        None
    }

    fn to_session(&self, _session_dir: &std::path::Path) -> Option<SessionPanel> {
        // Save file manager with current directory path
        Some(SessionPanel::FileManager {
            path: self.current_path.clone(),
        })
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
}

// Additional methods used by app layer (not part of Panel trait)
impl FileManager {
    /// Take modal window request (if any).
    pub fn take_modal_request(&mut self) -> Option<(PendingAction, ActiveModal)> {
        self.modal_request.take()
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
    fn test_handle_command_not_applicable() {
        let (mut fm, _temp_dir) = create_file_manager_in_temp();

        // Commands not applicable to FileManager should return None
        let result = fm.handle_command(PanelCommand::GetRepoRoot);
        assert!(matches!(result, CommandResult::None));

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
        // Title should contain the directory path
        assert!(fm.title().contains(&temp_dir.path().display().to_string()));
    }

    #[test]
    fn test_file_manager_panel_trait_needs_close_confirmation() {
        let (fm, _temp_dir) = create_file_manager_in_temp();
        // FileManager doesn't need close confirmation by default
        assert!(fm.needs_close_confirmation().is_none());
    }
}
